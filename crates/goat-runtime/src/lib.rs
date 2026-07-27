use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

mod embed;
use embed::OpenAiEmbedderAdapter;

use anyhow::{Context, Result};
use goat_agent_command::{CommandFactory, CommandProviderContext, CommandRegistry};
use goat_agent_tool::ToolRegistry;
use goat_brain::{Brain, BrainDeps, ProviderRegistry};
use goat_bus::EventBus;
use goat_channel::{Channel, ChannelBinding, ChannelFactory, ChannelHandle};
use goat_config::{GoatPaths, LoadedConfig};
use goat_memory::Embedder;
use goat_profile::ProfileConfig;
use goat_render::{DefaultStreamRenderer, StreamRenderer};
use goat_store::{SqliteStore, Store};
use goat_types::{Event, InstanceId, ProfileId};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub struct Goat {
    join_handles: Vec<tokio::task::JoinHandle<()>>,
    cancel: CancellationToken,
    _pty_manager: Arc<goat_agent_tool_pty::PtyManager>,
    _log_guard: Option<WorkerGuard>,
}

fn init_logging(logs_dir: &Path) -> WorkerGuard {
    std::fs::create_dir_all(logs_dir).ok();
    let file_appender = tracing_appender::rolling::daily(logs_dir, "goat");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let env =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,goat=debug"));
    let _ = tracing_subscriber::registry()
        .with(env)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(true))
        .with(
            fmt::layer()
                .with_writer(file_writer)
                .with_target(true)
                .json()
                .with_current_span(false),
        )
        .try_init();
    guard
}

impl Goat {
    pub async fn boot() -> Result<Self> {
        Self::boot_with_code(None).await
    }

    pub async fn boot_with_code(code: Option<goat_daemon::Manager>) -> Result<Self> {
        let paths = GoatPaths::default_layout().context("resolving ~/.goat layout")?;
        std::fs::create_dir_all(&paths.logs_dir).ok();
        let guard = init_logging(&paths.logs_dir);
        let cfg = goat_config::load_from(paths).context("loading config")?;
        Self::boot_inner(cfg, code, Some(guard)).await
    }

    async fn boot_inner(
        cfg: LoadedConfig,
        code: Option<goat_daemon::Manager>,
        log_guard: Option<WorkerGuard>,
    ) -> Result<Self> {
        info!(root = %cfg.paths.root.display(), "booting goat");

        let sqlite_store = SqliteStore::open(&cfg.paths.state_db)
            .await
            .context("open store")?;
        let pool_for_memory = sqlite_store.pool();
        let store: Arc<dyn Store> = Arc::new(sqlite_store);

        let sdk_store = goat_auth::CredentialStore::new(cfg.paths.credentials_json.clone());
        let providers = build_provider_registry(&sdk_store);
        let embedders = build_embedders(&cfg.agents, &sdk_store).await;
        let channels = build_channel_registry();

        let bus = EventBus::new();
        let (scheduler_handle, prepared_scheduler) =
            goat_loop::scheduler::prepare_scheduler(store.clone(), bus.clone())
                .await
                .context("prepare scheduler")?;

        let cancel = CancellationToken::new();

        let mut tools_reg = ToolRegistry::from_inventory();
        goat_agent_tool_schedule::register(&mut tools_reg, store.clone(), scheduler_handle);
        goat_agent_tool_goal::register(&mut tools_reg, store.clone());

        let mem_embedder: Option<Arc<dyn Embedder>> = embedders.values().next().cloned();
        let memory_engine = Arc::new(
            goat_memory::MemoryEngine::open(
                pool_for_memory.clone(),
                &cfg.paths.root,
                mem_embedder,
                180.0,
            )
            .await
            .context("open memory engine")?,
        );
        goat_agent_tool_memory::register(&mut tools_reg, memory_engine.clone());
        let pty_manager = Arc::new(goat_agent_tool_pty::PtyManager::new(
            cancel.clone(),
            goat_agent_tool_pty::MAX_SESSIONS,
        ));
        goat_agent_tool_pty::register(&mut tools_reg, pty_manager.clone());

        if let Some(manager) = code {
            goat_agent_tool_code::register(&mut tools_reg, manager);
        }
        let tools = Arc::new(tools_reg);
        info!(
            default_tools = tools.default_specs().len(),
            "loaded tool registry"
        );

        let renderer: Arc<dyn StreamRenderer> = Arc::new(DefaultStreamRenderer);

        let mut join_handles = Vec::new();

        let shared = RuntimeShared {
            providers: providers.clone(),
            channels: &channels,
            tools,
            goat_root: cfg.paths.root.clone(),
            store,
            memory_engine,
            renderer,
            bus,
            cancel: cancel.clone(),
        };

        for raw_profile in &cfg.agents {
            match spawn_profile(raw_profile, &shared).await {
                Ok(handles) => join_handles.extend(handles),
                Err(e) => warn!(profile = %raw_profile.slug, error = ?e, "skipping profile"),
            }
        }

        tokio::task::yield_now().await;
        join_handles.push(prepared_scheduler.spawn_with_cancel(cancel.clone()));

        {
            let engine = shared.memory_engine.clone();
            let providers = shared.providers.clone();
            let store = shared.store.clone();
            let profile_models: Vec<(ProfileId, goat_model::Model)> = cfg
                .agents
                .iter()
                .map(|p| (p.id, p.default_model.clone()))
                .collect();
            join_handles.push(goat_sleep::spawn(
                goat_sleep::SleepConfig::default(),
                cancel.clone(),
                move || {
                    let engine = engine.clone();
                    let providers = providers.clone();
                    let store = store.clone();
                    let profile_models = profile_models.clone();
                    async move {
                        if store.is_paused().await.unwrap_or(false) {
                            return;
                        }
                        run_consolidation(&engine, &providers, &store, &profile_models).await;
                    }
                },
            ));
        }

        Ok(Self {
            join_handles,
            cancel,
            _pty_manager: pty_manager,
            _log_guard: log_guard,
        })
    }

    pub async fn run(self) -> Result<()> {
        info!(handles = self.join_handles.len(), "goat running");
        let signal = shutdown_signal().await;
        info!(signal = signal, "shutdown signal received; shutting down");
        self.drain().await
    }

    pub async fn run_until(self, trigger: CancellationToken) -> Result<()> {
        info!(handles = self.join_handles.len(), "goat running");
        trigger.cancelled().await;
        info!("shutdown requested; shutting down");
        self.drain().await
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    async fn drain(mut self) -> Result<()> {
        self.cancel.cancel();
        let grace = std::time::Duration::from_secs(10);
        let handles = std::mem::take(&mut self.join_handles);
        let drain = futures::future::join_all(handles);
        if tokio::time::timeout(grace, drain).await.is_err() {
            warn!("shutdown grace period elapsed; detaching remaining tasks");
        }
        Ok(())
    }
}

async fn run_consolidation(
    engine: &Arc<goat_memory::MemoryEngine>,
    providers: &Arc<ProviderRegistry>,
    store: &Arc<dyn Store>,
    profile_models: &[(ProfileId, goat_model::Model)],
) {
    use goat_memory::Scope;
    for (persona, model) in profile_models {
        let provider = match providers.route(model) {
            Ok(p) => p,
            Err(e) => {
                warn!(profile = %persona, error = ?e, "sleep: no provider for model");
                continue;
            }
        };
        let conv = match store.latest_thread(*persona).await {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                warn!(profile = %persona, error = ?e, "sleep: latest_thread failed");
                continue;
            }
        };
        let rows = match store.recent(*persona, &conv, 200).await {
            Ok(r) => r,
            Err(e) => {
                warn!(profile = %persona, error = ?e, "sleep: recent failed");
                continue;
            }
        };
        let transcript: Vec<goat_sleep::TranscriptLine> = rows
            .into_iter()
            .map(|r| goat_sleep::TranscriptLine {
                role: match r.direction {
                    goat_store::Direction::In => "user",
                    goat_store::Direction::Out => "assistant",
                },
                text: r.text,
            })
            .collect();
        if let Err(e) =
            goat_sleep::run_once(engine, &provider, model, &Scope::Owner, &transcript).await
        {
            warn!(profile = %persona, error = ?e, "sleep: consolidation failed");
        }
    }
}

async fn shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => "ctrl_c",
            () = async {
                if let Some(stream) = terminate.as_mut() {
                    stream.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => "sigterm",
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
        "ctrl_c"
    }
}

fn build_provider_registry(store: &goat_auth::CredentialStore) -> Arc<ProviderRegistry> {
    use goat_auth::CredentialService;
    use goat_providers::{DEFAULT_ACCOUNT, Registry};

    let mut accounts: Vec<String> = store
        .entries()
        .into_iter()
        .filter(|(key, _)| key.service == CredentialService::Model)
        .map(|(key, _)| key.account)
        .collect();
    accounts.push(DEFAULT_ACCOUNT.to_owned());
    accounts.sort();
    accounts.dedup();

    let mut registry = ProviderRegistry::new();
    let mut logged = std::collections::HashSet::new();
    for account in &accounts {
        for provider in Registry::load(store, account).all() {
            if logged.insert(provider.id().to_string()) {
                info!(provider = %provider.id(), "loaded provider");
            }
            registry.insert_account(account.clone(), provider.clone());
        }
    }
    Arc::new(registry)
}

async fn build_embedders(
    agents: &[ProfileConfig],
    store: &goat_auth::CredentialStore,
) -> Arc<HashMap<ProfileId, Arc<dyn Embedder>>> {
    let mut map: HashMap<ProfileId, Arc<dyn Embedder>> = HashMap::new();
    for profile in agents {
        if !profile.memory.enabled {
            continue;
        }
        let Some(settings) = profile.memory.embedding.as_ref() else {
            continue;
        };
        if settings.provider != "openai" {
            warn!(
                profile = %profile.slug,
                provider = %settings.provider,
                "memory: unsupported embedding provider; episodic memory disabled for this profile",
            );
            continue;
        }
        match OpenAiEmbedderAdapter::new(store.clone(), settings.model.clone()).await {
            Ok(embedder) => {
                info!(
                    profile = %profile.slug,
                    provider = %settings.provider,
                    model = %settings.model,
                    dim = embedder.dim(),
                    "memory: embedder ready",
                );
                map.insert(profile.id, Arc::new(embedder));
            }
            Err(e) => warn!(
                profile = %profile.slug,
                provider = %settings.provider,
                model = %settings.model,
                error = ?e,
                "memory: embedding probe failed; episodic memory disabled for this profile",
            ),
        }
    }
    Arc::new(map)
}

fn build_channel_registry() -> HashMap<String, Arc<dyn Channel>> {
    let mut by_name: HashMap<String, Arc<dyn Channel>> = HashMap::new();
    for factory in inventory::iter::<ChannelFactory>() {
        let id = factory.id.as_str().to_string();
        if by_name.contains_key(&id) {
            warn!(
                channel = %id,
                "duplicate channel ID in inventory; first registration wins",
            );
            continue;
        }
        by_name.insert(id, (factory.ctor)());
    }
    by_name
}

struct RuntimeShared<'a> {
    providers: Arc<ProviderRegistry>,
    channels: &'a HashMap<String, Arc<dyn Channel>>,
    tools: Arc<ToolRegistry>,
    goat_root: std::path::PathBuf,
    store: Arc<dyn Store>,
    memory_engine: Arc<goat_memory::MemoryEngine>,
    renderer: Arc<dyn StreamRenderer>,
    bus: EventBus,
    cancel: CancellationToken,
}

async fn spawn_profile(
    raw: &ProfileConfig,
    shared: &RuntimeShared<'_>,
) -> Result<Vec<tokio::task::JoinHandle<()>>> {
    shared
        .providers
        .route(&raw.default_model)
        .with_context(|| format!("no provider for model {}", raw.default_model))?;
    shared
        .tools
        .validate_default_selectors(&raw.tool_selectors)
        .with_context(|| format!("invalid tools for persona {}", raw.slug))?;

    shared
        .store
        .ensure_persona(raw.id, &raw.slug, &raw.display)
        .await?;

    let mut handles: Vec<Arc<dyn ChannelHandle>> = Vec::new();
    let mut joins: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let commands = Arc::new(build_command_registry(&shared.goat_root, raw.id));
    let command_specs = commands.specs();

    for binding in &raw.bindings {
        let Some(channel) = shared.channels.get(binding.name.as_str()) else {
            warn!(
                profile = %raw.slug,
                binding = %binding.name,
                "skipping binding: no compiled-in channel/plugin with this name",
            );
            continue;
        };
        let instance_slug = format!("{}/{}/{}", raw.id, channel.id(), binding.name);
        let chan_binding = ChannelBinding {
            instance: InstanceId::from_slug(&instance_slug),
            config: binding.config.clone(),
            commands: command_specs.clone(),
        };
        match channel.clone().bind(raw.id, chan_binding).await {
            Ok((handle, mut rx)) => {
                let bus_for_pump = shared.bus.clone();
                let cancel_for_pump = shared.cancel.clone();
                joins.push(tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            biased;
                            () = cancel_for_pump.cancelled() => break,
                            msg = rx.recv() => match msg {
                                Some(m) => bus_for_pump.publish(Event::Incoming(m)),
                                None => break,
                            },
                        }
                    }
                }));
                handles.push(handle);
            }
            Err(e) => warn!(
                profile = %raw.slug,
                binding = %binding.name,
                error = ?e,
                "skipping binding: bind failed",
            ),
        }
    }

    if handles.is_empty() {
        anyhow::bail!("no successful channel bindings");
    }

    let brain = Arc::new(Brain::new(BrainDeps {
        persona: raw.id,
        personality: Arc::new(raw.personality.clone()),
        default_model: raw.default_model.clone(),
        history_window: raw.history_window,
        tool_selectors: raw.tool_selectors.clone(),
        providers: shared.providers.clone(),
        tools: shared.tools.clone(),
        commands,
        store: shared.store.clone(),
        memory_engine: shared.memory_engine.clone(),
        memory_enabled: raw.memory.enabled,
        summarize_enabled: raw.memory.summarize,
        renderer: shared.renderer.clone(),
        goat_root: shared.goat_root.clone(),
        stream_idle_timeout: std::time::Duration::from_mins(1),
        llm_max_retries: 3,
        intake_debounce: raw.intake_debounce,
        intake_ceiling: raw.intake_ceiling,
    }));
    let bus = shared.bus.clone();
    let cancel_for_brain = shared.cancel.clone();
    joins.push(tokio::spawn(async move {
        if let Err(e) = brain.run(bus, handles, cancel_for_brain).await {
            warn!(error = ?e, "brain exited");
        }
    }));

    Ok(joins)
}

fn build_command_registry(
    goat_root: &std::path::Path,
    persona: goat_types::ProfileId,
) -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    let ctx = CommandProviderContext::new(goat_root.to_path_buf(), persona);
    for factory in inventory::iter::<CommandFactory>() {
        (factory.register)(&mut registry, &ctx);
        info!(
            provider = factory.id,
            commands = registry.specs().len(),
            "loaded command provider"
        );
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_model::{Model, ProviderId};
    use goat_profile::{
        AutonomyConfig, EmbeddingSettings, MemoryConfig, ProfileCard, ProfileConfig,
    };

    fn paths_in(dir: &Path) -> GoatPaths {
        GoatPaths::from_root(dir.to_path_buf())
    }

    fn persona(slug: &str, model: &str) -> ProfileConfig {
        ProfileConfig {
            id: ProfileId::from_slug(slug),
            slug: slug.into(),
            display: slug.into(),
            personality: ProfileCard {
                system_prompt: "you are a test persona".into(),
                source_path: std::path::PathBuf::new(),
            },
            default_model: Model::new(ProviderId::from("openai"), model),
            history_window: 10,
            tool_selectors: vec![],
            bindings: vec![],
            memory: MemoryConfig::default(),
            autonomy: AutonomyConfig::default(),
            intake_debounce: std::time::Duration::from_secs(1),
            intake_ceiling: std::time::Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn boots_with_no_personas_and_spawns_only_scheduler() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            agents: vec![],
        };
        let goat = Goat::boot_inner(cfg, None, None).await.expect("boot");
        assert_eq!(
            goat.join_handles.len(),
            2,
            "expected the scheduler and sleep-job tasks"
        );
    }

    #[tokio::test]
    async fn persona_with_unresolvable_provider_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            agents: vec![persona("alice", "openai/gpt-5.1")],
        };
        let goat = Goat::boot_inner(cfg, None, None).await.expect("boot");
        assert_eq!(goat.join_handles.len(), 2);
    }

    #[tokio::test]
    async fn embedder_probe_failure_degrades_to_core_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = persona("bob", "openai/gpt-5.1");
        p.memory = MemoryConfig {
            enabled: true,
            embedding: Some(EmbeddingSettings {
                provider: "openai".into(),
                model: "text-embedding-3-small".into(),
            }),
            episodic_k: 8,
            summarize: false,
        };
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            agents: vec![p],
        };
        let goat = Goat::boot_inner(cfg, None, None).await.expect("boot");
        assert_eq!(goat.join_handles.len(), 2);
    }
}

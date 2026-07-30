use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

mod channel_secrets;
mod embed;
use embed::OpenAiEmbedderAdapter;

use anyhow::{Context, Result};
use goat_agent_command::{CommandFactory, CommandProviderContext, CommandRegistry};
use goat_agent_config::AgentConfig;
use goat_agent_tool::ToolRegistry;
use goat_brain::{Brain, BrainDeps, ProviderRegistry};
use goat_bus::EventBus;
use goat_channel::{Channel, ChannelBinding, ChannelFactory, ChannelHandle};
use goat_config::{GoatPaths, LoadedConfig};
use goat_integration::{Integration, IntegrationBinding, IntegrationRuntime};
use goat_memory::Embedder;
use goat_render::{DefaultStreamRenderer, StreamRenderer};
use goat_store::{SqliteStore, Store};
use goat_types::{AgentId, Event, InstanceId};
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
        Self::boot_with_code_metered(code, None).await
    }

    pub async fn boot_with_code_metered(
        code: Option<goat_daemon::Manager>,
        meter: Option<goat_proxy::Meter>,
    ) -> Result<Self> {
        let paths = GoatPaths::default_layout().context("resolving ~/.goat layout")?;
        std::fs::create_dir_all(&paths.logs_dir).ok();
        let guard = init_logging(&paths.logs_dir);
        let cfg = goat_config::load_from(paths).context("loading config")?;
        Self::boot_inner(cfg, code, meter, Some(guard)).await
    }

    async fn boot_inner(
        cfg: LoadedConfig,
        code: Option<goat_daemon::Manager>,
        meter: Option<goat_proxy::Meter>,
        log_guard: Option<WorkerGuard>,
    ) -> Result<Self> {
        info!(root = %cfg.paths.root.display(), "booting goat");

        let sqlite_store = SqliteStore::open(&cfg.paths.state_db)
            .await
            .context("open store")?;
        let pool_for_memory = sqlite_store.pool();
        let store: Arc<dyn Store> = Arc::new(sqlite_store);

        let sdk_store = goat_auth::CredentialStore::new(cfg.paths.credentials_json.clone());
        let providers = build_provider_registry(&sdk_store, meter.as_ref());
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
        goat_agent_tool_observation::register(&mut tools_reg, store.clone());

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

        let integrations = goat_integration::registry_from_inventory();
        let connections = load_integration_connections(&cfg.paths.config_json);
        let integration_bindings =
            build_integration_bindings(&cfg.agents, &integrations, &connections);
        let integration_runtime = IntegrationRuntime {
            credentials: sdk_store.clone(),
            store: store.clone(),
            bus: bus.clone(),
        };
        let mut integration_tool_names: HashMap<String, Vec<String>> = HashMap::new();
        for (id, integration) in &integrations {
            if let Some(bindings) = integration_bindings.get(id).filter(|b| !b.is_empty()) {
                let names = integration
                    .register_tools(&mut tools_reg, &integration_runtime, bindings.clone())
                    .await;
                integration_tool_names.insert(
                    id.clone(),
                    names.iter().map(|n| n.as_str().to_string()).collect(),
                );
            }
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
            integrations: &integrations,
            integration_bindings: &integration_bindings,
            integration_runtime,
            integration_tool_names,
            tools,
            goat_root: cfg.paths.root.clone(),
            agents_dir: cfg.paths.agents_dir.clone(),
            credentials: sdk_store.clone(),
            store,
            memory_engine,
            renderer,
            bus,
            cancel: cancel.clone(),
        };

        for raw_agent in &cfg.agents {
            match spawn_agent(raw_agent, &shared).await {
                Ok(handles) => join_handles.extend(handles),
                Err(e) => warn!(agent = %raw_agent.slug, error = ?e, "skipping agent"),
            }
        }

        tokio::task::yield_now().await;
        join_handles.push(prepared_scheduler.spawn_with_cancel(cancel.clone()));

        {
            let engine = shared.memory_engine.clone();
            let providers = shared.providers.clone();
            let store = shared.store.clone();
            let agent_models: Vec<(AgentId, goat_model::Model)> = cfg
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
                    let agent_models = agent_models.clone();
                    async move {
                        if store.is_paused().await.unwrap_or(false) {
                            return;
                        }
                        run_consolidation(&engine, &providers, &store, &agent_models).await;
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
    agent_models: &[(AgentId, goat_model::Model)],
) {
    use goat_memory::Scope;
    for (agent, model) in agent_models {
        let provider = match providers.route(model) {
            Ok(p) => p,
            Err(e) => {
                warn!(agent = %agent, error = ?e, "sleep: no provider for model");
                continue;
            }
        };
        let conv = match store.latest_thread(*agent).await {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                warn!(agent = %agent, error = ?e, "sleep: latest_thread failed");
                continue;
            }
        };
        let rows = match store.recent(*agent, &conv, 200).await {
            Ok(r) => r,
            Err(e) => {
                warn!(agent = %agent, error = ?e, "sleep: recent failed");
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
            warn!(agent = %agent, error = ?e, "sleep: consolidation failed");
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

fn build_provider_registry(
    store: &goat_auth::CredentialStore,
    meter: Option<&goat_proxy::Meter>,
) -> Arc<ProviderRegistry> {
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
        for provider in Registry::load_metered(store, account, meter.cloned()).all() {
            if logged.insert(provider.id().to_string()) {
                info!(provider = %provider.id(), "loaded provider");
            }
            registry.insert_account(account.clone(), provider.clone());
        }
    }
    Arc::new(registry)
}

async fn build_embedders(
    agents: &[AgentConfig],
    store: &goat_auth::CredentialStore,
) -> Arc<HashMap<AgentId, Arc<dyn Embedder>>> {
    let mut map: HashMap<AgentId, Arc<dyn Embedder>> = HashMap::new();
    for agent in agents {
        if !agent.memory.enabled {
            continue;
        }
        let Some(settings) = agent.memory.embedding.as_ref() else {
            continue;
        };
        if settings.provider != "openai" {
            warn!(
                agent = %agent.slug,
                provider = %settings.provider,
                "memory: unsupported embedding provider; episodic memory disabled for this agent",
            );
            continue;
        }
        match OpenAiEmbedderAdapter::new(store.clone(), settings.model.clone()).await {
            Ok(embedder) => {
                info!(
                    agent = %agent.slug,
                    provider = %settings.provider,
                    model = %settings.model,
                    dim = embedder.dim(),
                    "memory: embedder ready",
                );
                map.insert(agent.id, Arc::new(embedder));
            }
            Err(e) => warn!(
                agent = %agent.slug,
                provider = %settings.provider,
                model = %settings.model,
                error = ?e,
                "memory: embedding probe failed; episodic memory disabled for this agent",
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

fn load_integration_connections(
    config_json: &std::path::Path,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    std::fs::read_to_string(config_json)
        .ok()
        .and_then(|raw| serde_json::from_str::<goat_config::Config>(&raw).ok())
        .map(|config| config.integrations)
        .unwrap_or_default()
}

fn merge_binding_config(
    connection: Option<&serde_json::Value>,
    binding: &serde_json::Value,
) -> serde_json::Value {
    match (
        connection.and_then(serde_json::Value::as_object),
        binding.as_object(),
    ) {
        (Some(base), Some(over)) => {
            let mut merged = base.clone();
            merged.extend(over.clone());
            serde_json::Value::Object(merged)
        }
        (Some(base), None) => serde_json::Value::Object(base.clone()),
        (None, _) => binding.clone(),
    }
}

fn build_integration_bindings(
    agents: &[AgentConfig],
    integrations: &HashMap<String, Arc<dyn Integration>>,
    connections: &std::collections::BTreeMap<String, serde_json::Value>,
) -> HashMap<String, Arc<goat_integration::BindingMap>> {
    let mut maps: HashMap<String, goat_integration::BindingMap> = HashMap::new();
    for agent in agents {
        for agent_integration in &agent.integrations {
            let name = agent_integration.name.as_str();
            if !integrations.contains_key(name) {
                warn!(
                    agent = %agent.slug,
                    integration = %name,
                    "unknown integration in agent config",
                );
                continue;
            }
            let Some(factory) = goat_integration::factory_for(name) else {
                continue;
            };
            if let Err(e) = (factory.validate_config)(&agent_integration.config) {
                warn!(
                    agent = %agent.slug,
                    integration = %name,
                    error = %e,
                    "skipping integration binding: invalid config",
                );
                continue;
            }
            let merged = merge_binding_config(connections.get(name), &agent_integration.config);
            maps.entry(name.to_string())
                .or_default()
                .insert(agent.id, IntegrationBinding::from_config(merged));
        }
    }
    maps.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}

struct RuntimeShared<'a> {
    providers: Arc<ProviderRegistry>,
    channels: &'a HashMap<String, Arc<dyn Channel>>,
    integrations: &'a HashMap<String, Arc<dyn Integration>>,
    integration_bindings: &'a HashMap<String, Arc<goat_integration::BindingMap>>,
    integration_runtime: IntegrationRuntime,
    integration_tool_names: HashMap<String, Vec<String>>,
    tools: Arc<ToolRegistry>,
    goat_root: std::path::PathBuf,
    agents_dir: std::path::PathBuf,
    credentials: goat_auth::CredentialStore,
    store: Arc<dyn Store>,
    memory_engine: Arc<goat_memory::MemoryEngine>,
    renderer: Arc<dyn StreamRenderer>,
    bus: EventBus,
    cancel: CancellationToken,
}

async fn spawn_agent(
    raw: &AgentConfig,
    shared: &RuntimeShared<'_>,
) -> Result<Vec<tokio::task::JoinHandle<()>>> {
    shared
        .providers
        .route(&raw.default_model)
        .with_context(|| format!("no provider for model {}", raw.default_model))?;
    shared
        .tools
        .validate_default_selectors(&raw.tool_selectors)
        .with_context(|| format!("invalid tools for agent {}", raw.slug))?;

    shared
        .store
        .ensure_agent(raw.id, &raw.slug, &raw.display)
        .await?;

    let mut handles: Vec<Arc<dyn ChannelHandle>> = Vec::new();
    let mut joins: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let commands = Arc::new(build_command_registry(&shared.goat_root, raw.id));
    let command_specs = commands.specs();

    for binding in &raw.bindings {
        let Some(channel) = shared.channels.get(binding.name.as_str()) else {
            warn!(
                agent = %raw.slug,
                binding = %binding.name,
                "skipping binding: no compiled-in channel/plugin with this name",
            );
            continue;
        };
        let channel_id = channel.id();
        let instance_slug = format!("{}/{}/{}", raw.id, channel_id, binding.name);
        let mut config = binding.config.clone();
        let secrets = channel_secrets::resolve_for_binding(
            &shared.credentials,
            &shared.agents_dir,
            &raw.slug,
            &channel_id,
            &binding.name,
            goat_channel::secret_specs(channel_id.as_str()),
            &mut config,
        );
        let chan_binding = ChannelBinding {
            instance: InstanceId::from_slug(&instance_slug),
            config,
            commands: command_specs.clone(),
            secrets,
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
                agent = %raw.slug,
                binding = %binding.name,
                error = ?e,
                "skipping binding: bind failed",
            ),
        }
    }

    if handles.is_empty() {
        anyhow::bail!("no successful channel bindings");
    }

    for agent_integration in &raw.integrations {
        let Some(integration) = shared.integrations.get(agent_integration.name.as_str()) else {
            warn!(
                agent = %raw.slug,
                integration = %agent_integration.name,
                "skipping integration: no compiled-in integration with this name",
            );
            continue;
        };
        let Some(binding) = shared
            .integration_bindings
            .get(agent_integration.name.as_str())
            .and_then(|map| map.get(&raw.id))
        else {
            continue;
        };
        if let Some(handle) = integration.spawn_watcher(
            raw.id,
            binding.clone(),
            shared.integration_runtime.clone(),
            shared.cancel.clone(),
        ) {
            joins.push(handle);
        }
    }

    let brain = Arc::new(Brain::new(BrainDeps {
        agent: raw.id,
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
        integration_tools: raw
            .integrations
            .iter()
            .filter_map(|pi| shared.integration_tool_names.get(pi.name.as_str()))
            .flatten()
            .cloned()
            .collect(),
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
    agent: goat_types::AgentId,
) -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    let ctx = CommandProviderContext::new(goat_root.to_path_buf(), agent);
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
    use goat_agent_config::{
        AgentCard, AgentConfig, AutonomyConfig, EmbeddingSettings, MemoryConfig,
    };
    use goat_model::{Model, ProviderId};

    fn paths_in(dir: &Path) -> GoatPaths {
        GoatPaths::from_root(dir.to_path_buf())
    }

    fn agent(slug: &str, model: &str) -> AgentConfig {
        AgentConfig {
            id: AgentId::from_slug(slug),
            slug: slug.into(),
            display: slug.into(),
            personality: AgentCard {
                system_prompt: "you are a test agent".into(),
                source_path: std::path::PathBuf::new(),
            },
            default_model: Model::new(ProviderId::from("openai"), model),
            history_window: 10,
            tool_selectors: vec![],
            bindings: vec![],
            integrations: vec![],
            memory: MemoryConfig::default(),
            autonomy: AutonomyConfig::default(),
            intake_debounce: std::time::Duration::from_secs(1),
            intake_ceiling: std::time::Duration::from_secs(5),
        }
    }

    struct FakeIntegration;

    #[async_trait::async_trait]
    impl Integration for FakeIntegration {
        fn id(&self) -> goat_types::IntegrationId {
            goat_types::IntegrationId::from_static("fake")
        }

        fn metadata(&self) -> goat_integration::IntegrationMetadata {
            goat_integration::IntegrationMetadata {
                id: "fake",
                display: "Fake",
                auth: goat_integration::IntegrationAuth::Secret,
                secret_label: "key",
                env_var: None,
                setup: "none",
            }
        }

        async fn register_tools(
            &self,
            _registry: &mut ToolRegistry,
            _runtime: &IntegrationRuntime,
            _bindings: Arc<goat_integration::BindingMap>,
        ) -> Vec<goat_agent_tool::ToolName> {
            vec![goat_agent_tool::ToolName::from_static("fake")]
        }

        async fn verify(
            &self,
            _config: &serde_json::Value,
            _credentials: &goat_auth::CredentialStore,
        ) -> goat_integration::IntegrationResult<String> {
            Ok("fake".into())
        }
    }

    inventory::submit! {
        goat_integration::IntegrationFactory {
            id: goat_types::IntegrationId::from_static("fake"),
            ctor: || Arc::new(FakeIntegration),
            validate_config: |config| {
                if config.get("bad").is_some() {
                    Err(goat_integration::IntegrationError::Config("bad".into()))
                } else {
                    Ok(())
                }
            },
        }
    }

    #[test]
    fn integration_bindings_validate_and_group_by_agent() {
        let integrations = goat_integration::registry_from_inventory();
        assert!(integrations.contains_key("fake"));

        let mut good = agent("good", "gpt-x");
        good.integrations = vec![goat_agent_config::AgentIntegration {
            name: "fake".into(),
            config: serde_json::json!({ "account": "work" }),
        }];
        let mut bad = agent("bad", "gpt-x");
        bad.integrations = vec![
            goat_agent_config::AgentIntegration {
                name: "fake".into(),
                config: serde_json::json!({ "bad": true }),
            },
            goat_agent_config::AgentIntegration {
                name: "unknown".into(),
                config: serde_json::json!({}),
            },
        ];

        let connections = std::collections::BTreeMap::from([(
            "fake".to_string(),
            serde_json::json!({ "client_id": "shared-client" }),
        )]);
        let maps = build_integration_bindings(&[good.clone(), bad], &integrations, &connections);
        let fake = maps.get("fake").expect("fake map");
        assert_eq!(fake.len(), 1);
        let binding = fake.get(&good.id).expect("good binding");
        assert_eq!(binding.account, "work");
        assert_eq!(binding.config["client_id"], "shared-client");
        assert!(!maps.contains_key("unknown"));
    }

    #[tokio::test]
    async fn boots_with_no_agents_and_spawns_only_scheduler() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            agents: vec![],
        };
        let goat = Goat::boot_inner(cfg, None, None, None).await.expect("boot");
        assert_eq!(
            goat.join_handles.len(),
            2,
            "expected the scheduler and sleep-job tasks"
        );
    }

    #[tokio::test]
    async fn agent_with_unresolvable_provider_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            agents: vec![agent("alice", "openai/gpt-5.1")],
        };
        let goat = Goat::boot_inner(cfg, None, None, None).await.expect("boot");
        assert_eq!(goat.join_handles.len(), 2);
    }

    #[tokio::test]
    async fn embedder_probe_failure_degrades_to_core_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = agent("bob", "openai/gpt-5.1");
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
        let goat = Goat::boot_inner(cfg, None, None, None).await.expect("boot");
        assert_eq!(goat.join_handles.len(), 2);
    }
}

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

mod channel_secrets;
mod embed;
mod layout;
use embed::OpenAiEmbedderAdapter;

use anyhow::{Context, Result};
use goat_agent_command::{CommandFactory, CommandProviderContext, CommandRegistry};
use goat_agent_config::AgentConfig;
use goat_agent_tool::ToolRegistry;
use goat_brain::{Brain, BrainDeps, ProviderRegistry};
use goat_bus::EventBus;
use goat_channel::{Channel, ChannelBinding, ChannelFactory, ChannelHandle};
use goat_config::{GoatPaths, LoadedConfig};
use goat_integration::watch::{Workflow, WorkflowSource, run_workflow};
use goat_integration::{Integration, IntegrationBinding, IntegrationRuntime, WatchSpec};
use goat_memory::Embedder;
use goat_render::{DefaultStreamRenderer, StreamRenderer};
use goat_store::{SqliteStore, Store};
use goat_types::{AgentId, Event, InstanceId};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub struct AgentRuntime {
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

impl AgentRuntime {
    pub async fn boot() -> Result<Self> {
        Self::boot_with_code(None).await
    }

    pub async fn boot_with_code(code: Option<goat_daemon::CodeSessionHub>) -> Result<Self> {
        Self::boot_with_code_metered(code, None).await
    }

    pub async fn boot_with_code_metered(
        code: Option<goat_daemon::CodeSessionHub>,
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
        code: Option<goat_daemon::CodeSessionHub>,
        meter: Option<goat_proxy::Meter>,
        log_guard: Option<WorkerGuard>,
    ) -> Result<Self> {
        info!(root = %cfg.paths.root.display(), "booting goat");
        layout::migrate(&cfg.paths);

        let sqlite_store = SqliteStore::open(&cfg.paths.state_db)
            .await
            .context("open store")?;
        let store: Arc<dyn Store> = Arc::new(sqlite_store);

        let credentials = goat_auth::CredentialStore::new(cfg.paths.credentials_json.clone());
        let user_providers = goat_config::UserProviders::at(cfg.paths.config_json.clone());
        let embedders = build_embedders(&cfg.agents, &credentials).await;

        let bus = EventBus::new();
        let (scheduler_handle, prepared_scheduler) =
            goat_loop::scheduler::prepare_scheduler(store.clone(), bus.clone())
                .await
                .context("prepare scheduler")?;

        let cancel = CancellationToken::new();

        let mem_embedder: Option<Arc<dyn Embedder>> = embedders.values().next().cloned();
        let memory_engine = Arc::new(
            goat_memory::MemoryEngine::open(
                &cfg.paths.state_db,
                &cfg.paths.root,
                mem_embedder,
                180.0,
            )
            .await
            .context("open memory engine")?,
        );
        let pty_manager = Arc::new(goat_agent_tool_pty::PtyManager::new(
            cancel.clone(),
            goat_agent_tool_pty::MAX_SESSIONS,
        ));

        let agent_turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        if let Some(manager) = &code {
            manager.set_agent_turns(agent_turns.clone());
        }

        let base = RuntimeBase {
            paths: cfg.paths.clone(),
            store: store.clone(),
            credentials,
            user_providers,
            meter,
            memory_engine: memory_engine.clone(),
            pty_manager: pty_manager.clone(),
            scheduler_handle,
            code: code.clone(),
            bus,
            renderer: Arc::new(DefaultStreamRenderer),
            agent_turns,
        };

        let shared = build_shared(&base, &cfg.agents).await;
        let providers = shared.providers.clone();

        let mut supervisor = Supervisor {
            base,
            shared,
            agents: HashMap::new(),
            shared_key: shared_fingerprint(&cfg.paths.config_json, &cfg.agents),
            cancel: cancel.clone(),
            models: Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        supervisor.sync(cfg.agents, None).await;

        let mut join_handles = Vec::new();
        tokio::task::yield_now().await;
        join_handles.push(prepared_scheduler.spawn_with_cancel(cancel.clone()));

        {
            let engine = memory_engine.clone();
            let store = store.clone();
            let models = supervisor.models.clone();
            join_handles.push(goat_sleep::spawn(
                goat_sleep::SleepConfig::default(),
                cancel.clone(),
                move || {
                    let engine = engine.clone();
                    let providers = providers.clone();
                    let store = store.clone();
                    let models = models.clone();
                    async move {
                        if store.is_paused().await.unwrap_or(false) {
                            return;
                        }
                        let agent_models = models.lock().map(|m| m.clone()).unwrap_or_default();
                        run_consolidation(&engine, &providers, &store, &agent_models).await;
                    }
                },
            ));
        }

        let (reload_tx, reload_rx) = tokio::sync::mpsc::channel(4);
        if let Some(manager) = code.as_ref() {
            manager.set_reload(reload_tx);
        }
        join_handles.push(tokio::spawn(supervisor.run(reload_rx)));

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
        let conv = match store.latest_conversation(*agent).await {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                warn!(agent = %agent, error = ?e, "sleep: latest_conversation failed");
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
    user: &goat_config::UserProviders,
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
        for provider in Registry::load_metered(store, user, account, meter.cloned()).all() {
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

#[derive(Clone, Debug)]
pub struct WatchIssue {
    pub workflow: String,
    pub source: String,
    pub stream: String,
    pub reason: String,
}

impl std::fmt::Display for WatchIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "watch `{}` source `{}` (stream `{}`): {}",
            self.workflow, self.source, self.stream, self.reason
        )
    }
}

struct ResolvedSource {
    workflow: String,
    name: String,
    integration: Arc<dyn Integration>,
    binding: IntegrationBinding,
    spec: WatchSpec,
}

type IntegrationMap = HashMap<String, Arc<dyn Integration>>;
type BindingMaps = HashMap<String, Arc<goat_integration::BindingMap>>;

fn declared_watch(
    raw: &AgentConfig,
    integrations: &IntegrationMap,
    bindings: &BindingMaps,
) -> Vec<(String, Vec<(String, WatchSpec)>)> {
    match &raw.watch {
        Some(workflows) => workflows
            .iter()
            .map(|workflow| {
                let sources = workflow
                    .sources
                    .iter()
                    .map(|entry| {
                        let stream = entry
                            .stream
                            .clone()
                            .unwrap_or_else(|| workflow.name.clone());
                        (
                            entry.source.clone(),
                            WatchSpec {
                                stream,
                                query: entry.query.clone(),
                            },
                        )
                    })
                    .collect();
                (workflow.name.clone(), sources)
            })
            .collect(),
        None => raw
            .integrations
            .iter()
            .filter_map(|agent_integration| {
                let name = agent_integration.name.as_str();
                let integration = integrations.get(name)?;
                let binding = bindings.get(name).and_then(|map| map.get(&raw.id))?;
                Some((name.to_string(), integration.default_watch(binding)))
            })
            .flat_map(|(name, specs)| {
                specs
                    .into_iter()
                    .map(move |spec| (spec.stream.clone(), vec![(name.clone(), spec)]))
            })
            .collect(),
    }
}

fn resolve_watch_sources(
    raw: &AgentConfig,
    integrations: &IntegrationMap,
    bindings: &BindingMaps,
) -> (Vec<ResolvedSource>, Vec<WatchIssue>) {
    let mut seen: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();
    let mut resolved = Vec::new();
    let mut issues = Vec::new();
    for (workflow, sources) in declared_watch(raw, integrations, bindings) {
        for (name, spec) in sources {
            let mut reject = |reason: &str| {
                issues.push(WatchIssue {
                    workflow: workflow.clone(),
                    source: name.clone(),
                    stream: spec.stream.clone(),
                    reason: reason.to_owned(),
                });
            };
            let Some(integration) = integrations.get(&name) else {
                reject("no compiled-in integration with this name");
                continue;
            };
            let Some(binding) = bindings.get(&name).and_then(|map| map.get(&raw.id)) else {
                reject("the integration is not bound to this agent");
                continue;
            };
            let key = (name.clone(), binding.account.clone(), spec.stream.clone());
            if !seen.insert(key) {
                reject("duplicate stream; name it explicitly with `stream`");
                continue;
            }
            resolved.push(ResolvedSource {
                workflow: workflow.clone(),
                name,
                integration: integration.clone(),
                binding: binding.clone(),
                spec,
            });
        }
    }
    (resolved, issues)
}

pub fn validate_agents(cfg: &LoadedConfig) -> Vec<(String, Vec<WatchIssue>)> {
    let integrations = goat_integration::registry_from_inventory();
    let connections = load_integration_connections(&cfg.paths.config_json);
    let bindings = build_integration_bindings(&cfg.agents, &integrations, &connections);
    cfg.agents
        .iter()
        .map(|agent| {
            (
                agent.slug.clone(),
                validate_watch(agent, &integrations, &bindings),
            )
        })
        .collect()
}

pub fn validate_watch(
    raw: &AgentConfig,
    integrations: &IntegrationMap,
    bindings: &BindingMaps,
) -> Vec<WatchIssue> {
    let (resolved, mut issues) = resolve_watch_sources(raw, integrations, bindings);
    for source in resolved {
        let Some(vocabulary) = source.integration.watch_vocabulary() else {
            issues.push(WatchIssue {
                workflow: source.workflow,
                source: source.name,
                stream: source.spec.stream,
                reason: "this integration does not support watch queries".to_owned(),
            });
            continue;
        };
        if let Err(e) = goat_integration::query::validate(vocabulary, &source.spec.query) {
            issues.push(WatchIssue {
                workflow: source.workflow,
                source: source.name,
                stream: source.spec.stream,
                reason: e.to_string(),
            });
        }
    }
    issues
}

fn build_watch_plan(raw: &AgentConfig, shared: &RuntimeShared) -> (Vec<Workflow>, Vec<WatchIssue>) {
    let (resolved, mut issues) =
        resolve_watch_sources(raw, &shared.integrations, &shared.integration_bindings);
    let mut plan: Vec<(String, Vec<WorkflowSource>)> = Vec::new();
    for source in resolved {
        let compiled = match source.integration.compile_watch(
            &source.binding,
            &shared.integration_runtime,
            &source.spec,
        ) {
            Ok(compiled) => compiled,
            Err(e) => {
                issues.push(WatchIssue {
                    workflow: source.workflow,
                    source: source.name,
                    stream: source.spec.stream,
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let entry = WorkflowSource {
            integration: source.integration.id(),
            account: source.binding.account,
            stream: source.spec.stream,
            compiled,
        };
        match plan.iter_mut().find(|(name, _)| *name == source.workflow) {
            Some((_, sources)) => sources.push(entry),
            None => plan.push((source.workflow, vec![entry])),
        }
    }
    let workflows = plan
        .into_iter()
        .map(|(name, sources)| Workflow::new(name, sources))
        .collect();
    (workflows, issues)
}

struct RuntimeShared {
    providers: Arc<ProviderRegistry>,
    channels: Arc<HashMap<String, Arc<dyn Channel>>>,
    integrations: Arc<IntegrationMap>,
    integration_bindings: Arc<BindingMaps>,
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
    agent_turns: Arc<std::sync::atomic::AtomicUsize>,
}

struct RuntimeBase {
    paths: GoatPaths,
    store: Arc<dyn Store>,
    credentials: goat_auth::CredentialStore,
    user_providers: goat_config::UserProviders,
    meter: Option<goat_proxy::Meter>,
    memory_engine: Arc<goat_memory::MemoryEngine>,
    pty_manager: Arc<goat_agent_tool_pty::PtyManager>,
    scheduler_handle: goat_loop::scheduler::SchedulerHandle,
    code: Option<goat_daemon::CodeSessionHub>,
    bus: EventBus,
    renderer: Arc<dyn StreamRenderer>,
    agent_turns: Arc<std::sync::atomic::AtomicUsize>,
}

async fn build_shared(base: &RuntimeBase, agents: &[AgentConfig]) -> RuntimeShared {
    let providers =
        build_provider_registry(&base.credentials, &base.user_providers, base.meter.as_ref());
    let channels = build_channel_registry();

    let mut tools_reg = ToolRegistry::from_inventory();
    goat_agent_tool_schedule::register(
        &mut tools_reg,
        base.store.clone(),
        base.scheduler_handle.clone(),
    );
    goat_agent_tool_goal::register(&mut tools_reg, base.store.clone());
    goat_agent_tool_observation::register(&mut tools_reg, base.store.clone());
    goat_agent_tool_memory::register(&mut tools_reg, base.memory_engine.clone());
    goat_agent_tool_pty::register(&mut tools_reg, base.pty_manager.clone());
    if let Some(manager) = base.code.clone() {
        goat_agent_tool_code::register(&mut tools_reg, manager);
    }

    let integrations = goat_integration::registry_from_inventory();
    let connections = load_integration_connections(&base.paths.config_json);
    let integration_bindings = build_integration_bindings(agents, &integrations, &connections);
    let integration_runtime = IntegrationRuntime::new(
        base.credentials.clone(),
        base.store.clone(),
        base.bus.clone(),
    );
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

    RuntimeShared {
        providers,
        channels: Arc::new(channels),
        integrations: Arc::new(integrations),
        integration_bindings: Arc::new(integration_bindings),
        integration_runtime,
        integration_tool_names,
        tools,
        goat_root: base.paths.root.clone(),
        agents_dir: base.paths.agents_dir.clone(),
        credentials: base.credentials.clone(),
        store: base.store.clone(),
        memory_engine: base.memory_engine.clone(),
        renderer: base.renderer.clone(),
        bus: base.bus.clone(),
        agent_turns: base.agent_turns.clone(),
    }
}

fn shared_fingerprint(config_json: &Path, agents: &[AgentConfig]) -> String {
    let raw = std::fs::read_to_string(config_json).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    let pick = |key: &str| parsed.get(key).cloned().unwrap_or(serde_json::Value::Null);
    let mut bound: Vec<&str> = agents
        .iter()
        .flat_map(|agent| agent.integrations.iter().map(|i| i.name.as_str()))
        .collect();
    bound.sort_unstable();
    bound.dedup();
    serde_json::json!({
        "integrations": pick("integrations"),
        "providers": pick("providers"),
        "bound": bound,
    })
    .to_string()
}

fn agent_fingerprint(agents_dir: &Path, slug: &str) -> String {
    std::fs::read_to_string(agents_dir.join(slug).join("config.json")).unwrap_or_default()
}

fn declared_agents(agents_dir: &Path) -> std::collections::HashSet<String> {
    let mut slugs = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir(agents_dir) else {
        return slugs;
    };
    for entry in entries.flatten() {
        if !entry
            .path()
            .join(goat_config::AGENT_DEFINITION_FILE)
            .exists()
        {
            continue;
        }
        if let Some(slug) = entry.file_name().to_str() {
            slugs.insert(slug.to_owned());
        }
    }
    slugs
}

struct AgentTasks {
    cancel: CancellationToken,
    joins: Vec<tokio::task::JoinHandle<()>>,
    fingerprint: String,
}

struct Supervisor {
    base: RuntimeBase,
    shared: RuntimeShared,
    agents: HashMap<String, AgentTasks>,
    shared_key: String,
    cancel: CancellationToken,
    models: Arc<std::sync::Mutex<Vec<(AgentId, goat_model::Model)>>>,
}

async fn stop_agent(tasks: AgentTasks) {
    tasks.cancel.cancel();
    let grace = std::time::Duration::from_secs(5);
    let drain = futures::future::join_all(tasks.joins);
    if tokio::time::timeout(grace, drain).await.is_err() {
        warn!("agent teardown grace elapsed; detaching remaining tasks");
    }
}

impl Supervisor {
    async fn sync(
        &mut self,
        agents: Vec<AgentConfig>,
        only: Option<&str>,
    ) -> goat_wire::ReloadReport {
        let mut report = goat_wire::ReloadReport::default();

        if only.is_none() {
            let wanted: std::collections::HashSet<&str> =
                agents.iter().map(|a| a.slug.as_str()).collect();
            let stale: Vec<String> = self
                .agents
                .keys()
                .filter(|slug| !wanted.contains(slug.as_str()))
                .cloned()
                .collect();
            let on_disk = declared_agents(&self.base.paths.agents_dir);
            for slug in stale {
                if on_disk.contains(&slug) {
                    warn!(agent = %slug, "config did not load; keeping the running agent");
                    report.failed.push(goat_wire::ReloadFailure {
                        agent: slug,
                        reason: "config did not load; keeping the settings already running"
                            .to_owned(),
                    });
                    continue;
                }
                if let Some(tasks) = self.agents.remove(&slug) {
                    stop_agent(tasks).await;
                    info!(agent = %slug, "agent removed from config; stopped");
                    report
                        .warnings
                        .push(format!("{slug}: no longer in config; stopped"));
                }
            }
        }

        for agent in &agents {
            if only.is_some_and(|slug| slug != agent.slug) {
                continue;
            }
            let fingerprint = agent_fingerprint(&self.base.paths.agents_dir, &agent.slug);
            let live = self.agents.get(&agent.slug);
            if live.is_some_and(|tasks| tasks.fingerprint == fingerprint) {
                report.unchanged.push(agent.slug.clone());
                continue;
            }
            if let Some(tasks) = self.agents.remove(&agent.slug) {
                stop_agent(tasks).await;
            }
            for issue in validate_watch(
                agent,
                &self.shared.integrations,
                &self.shared.integration_bindings,
            ) {
                report.warnings.push(format!("{}: {issue}", agent.slug));
            }
            let cancel = self.cancel.child_token();
            match spawn_agent(agent, &self.shared, &cancel).await {
                Ok(joins) => {
                    self.agents.insert(
                        agent.slug.clone(),
                        AgentTasks {
                            cancel,
                            joins,
                            fingerprint,
                        },
                    );
                    report.reloaded.push(agent.slug.clone());
                }
                Err(e) => {
                    cancel.cancel();
                    warn!(agent = %agent.slug, error = ?e, "skipping agent");
                    report.failed.push(goat_wire::ReloadFailure {
                        agent: agent.slug.clone(),
                        reason: format!("{e:#}"),
                    });
                }
            }
        }

        if let Some(slug) = only
            && !agents.iter().any(|a| a.slug == slug)
        {
            report.failed.push(goat_wire::ReloadFailure {
                agent: slug.to_owned(),
                reason: "no agent with this name loaded from config".to_owned(),
            });
        }

        if let Ok(mut models) = self.models.lock() {
            *models = agents
                .iter()
                .map(|a| (a.id, a.default_model.clone()))
                .collect();
        }
        report
    }

    async fn reload(&mut self, only: Option<String>) -> goat_wire::ReloadReport {
        let cfg = match goat_config::load_from(self.base.paths.clone()) {
            Ok(cfg) => cfg,
            Err(e) => {
                return goat_wire::ReloadReport {
                    failed: vec![goat_wire::ReloadFailure {
                        agent: "*".to_owned(),
                        reason: format!("{e:#}"),
                    }],
                    ..Default::default()
                };
            }
        };

        let shared_key = shared_fingerprint(&self.base.paths.config_json, &cfg.agents);
        let mut only = only;
        if shared_key == self.shared_key {
            let bindings = build_integration_bindings(
                &cfg.agents,
                &self.shared.integrations,
                &load_integration_connections(&self.base.paths.config_json),
            );
            self.shared.integration_bindings = Arc::new(bindings);
        } else {
            info!("shared configuration changed; rebuilding providers, integrations, and tools");
            self.shared = build_shared(&self.base, &cfg.agents).await;
            self.shared_key = shared_key;
            only = None;
            let live: Vec<String> = self.agents.keys().cloned().collect();
            for slug in live {
                if let Some(tasks) = self.agents.remove(&slug) {
                    stop_agent(tasks).await;
                }
            }
        }

        self.sync(cfg.agents, only.as_deref()).await
    }

    async fn run(mut self, mut requests: tokio::sync::mpsc::Receiver<goat_daemon::ReloadRequest>) {
        loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => break,
                request = requests.recv() => match request {
                    Some(request) => {
                        let report = self.reload(request.agent).await;
                        let _ = request.reply.send(report);
                    }
                    None => break,
                },
            }
        }
        let joins: Vec<tokio::task::JoinHandle<()>> = self
            .agents
            .drain()
            .flat_map(|(_, tasks)| tasks.joins)
            .collect();
        let grace = std::time::Duration::from_secs(5);
        if tokio::time::timeout(grace, futures::future::join_all(joins))
            .await
            .is_err()
        {
            warn!("agent drain grace elapsed; detaching remaining tasks");
        }
    }
}

async fn spawn_agent(
    raw: &AgentConfig,
    shared: &RuntimeShared,
    cancel: &CancellationToken,
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
                let cancel_for_pump = cancel.clone();
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

    let (workflows, issues) = build_watch_plan(raw, shared);
    for issue in &issues {
        warn!(agent = %raw.slug, issue = %issue, "skipping watch source");
    }
    for workflow in workflows {
        joins.push(tokio::spawn(run_workflow(
            workflow,
            raw.id,
            shared.integration_runtime.clone(),
            cancel.clone(),
        )));
    }

    let brain = Arc::new(Brain::new(BrainDeps {
        agent: raw.id,
        personality: Arc::new(raw.personality.clone()),
        default_model: raw.default_model.clone(),
        timezone: raw.timezone.clone(),
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
        turns: shared.agent_turns.clone(),
    }));
    let bus = shared.bus.clone();
    let cancel_for_brain = cancel.clone();
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
            timezone: None,
            history_window: 10,
            tool_selectors: vec![],
            bindings: vec![],
            integrations: vec![],
            watch: None,
            memory: MemoryConfig::default(),
            autonomy: AutonomyConfig::default(),
            intake_debounce: std::time::Duration::from_secs(1),
            intake_ceiling: std::time::Duration::from_secs(5),
        }
    }

    static FAKE_VOCABULARY: goat_integration::query::WatchVocabulary =
        goat_integration::query::WatchVocabulary {
            integration: "fake",
            residue: goat_integration::query::Residue::Reject,
            terms: goat_integration::query::TermPolicy::Reject,
            limit: None,
            keys: &[goat_integration::query::KeySpec::new("assignee").selfref()],
        };

    struct FakeIntegration;

    #[async_trait::async_trait]
    impl Integration for FakeIntegration {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn id(&self) -> goat_types::IntegrationId {
            goat_types::IntegrationId::from_static("fake")
        }

        fn watch_vocabulary(&self) -> Option<&'static goat_integration::query::WatchVocabulary> {
            Some(&FAKE_VOCABULARY)
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

    fn watching(slug: &str, query: &str) -> AgentConfig {
        let mut agent = agent(slug, "gpt-x");
        agent.integrations = vec![goat_agent_config::AgentIntegration {
            name: "fake".into(),
            config: serde_json::json!({ "account": "work" }),
        }];
        agent.watch = Some(vec![goat_agent_config::WatchWorkflow {
            name: "inbox".into(),
            sources: vec![goat_agent_config::WatchSourceEntry {
                source: "fake".into(),
                query: query.into(),
                stream: None,
            }],
        }]);
        agent
    }

    fn issues_for(agent: &AgentConfig) -> Vec<WatchIssue> {
        let integrations = goat_integration::registry_from_inventory();
        let bindings = build_integration_bindings(
            std::slice::from_ref(agent),
            &integrations,
            &std::collections::BTreeMap::new(),
        );
        validate_watch(agent, &integrations, &bindings)
    }

    #[test]
    fn a_watch_query_the_vocabulary_accepts_raises_nothing() {
        assert!(issues_for(&watching("good", "assignee:@me")).is_empty());
    }

    #[test]
    fn a_watch_query_with_an_unknown_key_is_reported_against_its_workflow() {
        let issues = issues_for(&watching("bad", "squad:core"));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].workflow, "inbox");
        assert_eq!(issues[0].source, "fake");
        assert_eq!(issues[0].stream, "inbox");
        assert!(issues[0].reason.contains("squad"), "{}", issues[0].reason);
    }

    #[test]
    fn an_agent_whose_config_stopped_loading_is_not_treated_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        std::fs::create_dir_all(agents_dir.join("alice")).unwrap();
        std::fs::write(agents_dir.join("alice").join("agent.md"), "You are alice.").unwrap();
        std::fs::write(agents_dir.join("alice").join("config.json"), "{ oops").unwrap();
        std::fs::create_dir_all(agents_dir.join("gone")).unwrap();

        let on_disk = declared_agents(&agents_dir);
        assert!(
            on_disk.contains("alice"),
            "a directory with agent.md is declared even when its config is broken",
        );
        assert!(
            !on_disk.contains("gone"),
            "a directory without agent.md is not an agent",
        );
    }

    #[test]
    fn a_watch_source_naming_an_unbound_integration_is_reported() {
        let mut agent = watching("unbound", "assignee:@me");
        agent.integrations.clear();
        let issues = issues_for(&agent);
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].reason.contains("not bound"),
            "{}",
            issues[0].reason
        );
    }

    #[tokio::test]
    async fn boots_with_no_agents_and_spawns_only_scheduler() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            agents: vec![],
        };
        let goat = AgentRuntime::boot_inner(cfg, None, None, None)
            .await
            .expect("boot");
        assert_eq!(
            goat.join_handles.len(),
            3,
            "expected the scheduler, sleep-job, and supervisor tasks"
        );
    }

    #[tokio::test]
    async fn agent_with_unresolvable_provider_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            agents: vec![agent("alice", "openai/gpt-5.1")],
        };
        let goat = AgentRuntime::boot_inner(cfg, None, None, None)
            .await
            .expect("boot");
        assert_eq!(goat.join_handles.len(), 3);
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
        let goat = AgentRuntime::boot_inner(cfg, None, None, None)
            .await
            .expect("boot");
        assert_eq!(goat.join_handles.len(), 3);
    }
}

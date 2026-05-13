use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use goat_brain::{Brain, ProviderRegistry};
use goat_bus::EventBus;
use goat_channel::{Channel, ChannelBinding, ChannelFactory, ChannelHandle};
use goat_command::{CommandFactory, CommandProviderContext, CommandRegistry};
use goat_config::{GoatPaths, LoadedConfig};
use goat_credentials::KeyPool;
use goat_llm::{KeyProvider, LlmProviderFactory};
use goat_persona::PersonaConfig;
use goat_render::{DefaultStreamRenderer, StreamRenderer};
use goat_store::{SqliteStore, Store};
use goat_tool::ToolRegistry;
use goat_types::{Event, InstanceId};
use tracing::{info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub struct Goat {
    join_handles: Vec<tokio::task::JoinHandle<()>>,
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
        let paths = GoatPaths::default_layout().context("resolving ~/.goat layout")?;
        std::fs::create_dir_all(&paths.logs_dir).ok();
        let guard = init_logging(&paths.logs_dir);
        let cfg = goat_config::load_from(paths)
            .await
            .context("loading config")?;
        Self::boot_inner(cfg, Some(guard)).await
    }

    async fn boot_inner(cfg: LoadedConfig, log_guard: Option<WorkerGuard>) -> Result<Self> {
        info!(root = %cfg.paths.root.display(), "booting goat");

        let store: Arc<dyn Store> = Arc::new(
            SqliteStore::open(&cfg.paths.state_db)
                .await
                .context("open store")?,
        );

        let keys: Arc<dyn KeyProvider> = Arc::new(KeyPool::from_credentials(&cfg.credentials));
        let providers = build_provider_registry(keys);
        let channels = build_channel_registry();
        let tools = Arc::new(ToolRegistry::from_inventory());
        info!(
            default_tools = tools.default_specs().len(),
            "loaded tool registry"
        );

        let bus = EventBus::new();
        let renderer: Arc<dyn StreamRenderer> = Arc::new(DefaultStreamRenderer);

        let mut join_handles = Vec::new();

        let shared = RuntimeShared {
            providers: providers.clone(),
            channels: &channels,
            tools,
            goat_root: cfg.paths.root.clone(),
            store,
            renderer,
            bus,
        };

        for raw_persona in &cfg.personas {
            match spawn_persona(raw_persona, &shared).await {
                Ok(handles) => join_handles.extend(handles),
                Err(e) => warn!(persona = %raw_persona.slug, error = ?e, "skipping persona"),
            }
        }

        Ok(Self {
            join_handles,
            _log_guard: log_guard,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        info!(handles = self.join_handles.len(), "goat running");
        tokio::signal::ctrl_c().await.ok();
        info!("ctrl-c received; shutting down");
        for h in self.join_handles.drain(..) {
            h.abort();
        }
        Ok(())
    }
}

fn build_provider_registry(keys: Arc<dyn KeyProvider>) -> Arc<ProviderRegistry> {
    let mut reg = ProviderRegistry::new();
    for factory in inventory::iter::<LlmProviderFactory>() {
        let provider = (factory.ctor)(keys.clone());
        info!(provider = factory.id.as_str(), "loaded provider");
        reg.insert(provider);
    }
    Arc::new(reg)
}

fn build_channel_registry() -> HashMap<String, Arc<dyn Channel>> {
    let mut by_name: HashMap<String, Arc<dyn Channel>> = HashMap::new();
    for factory in inventory::iter::<ChannelFactory>() {
        by_name
            .entry(factory.id.as_str().to_string())
            .or_insert_with(|| (factory.ctor)());
    }
    by_name
}

struct RuntimeShared<'a> {
    providers: Arc<ProviderRegistry>,
    channels: &'a HashMap<String, Arc<dyn Channel>>,
    tools: Arc<ToolRegistry>,
    goat_root: std::path::PathBuf,
    store: Arc<dyn Store>,
    renderer: Arc<dyn StreamRenderer>,
    bus: EventBus,
}

async fn spawn_persona(
    raw: &PersonaConfig,
    shared: &RuntimeShared<'_>,
) -> Result<Vec<tokio::task::JoinHandle<()>>> {
    shared
        .providers
        .route(&raw.default_model)
        .with_context(|| format!("no provider for model {}", raw.default_model))?;

    shared
        .store
        .ensure_persona(raw.id, &raw.slug, &raw.display)
        .await?;

    let mut handles: Vec<Arc<dyn ChannelHandle>> = Vec::new();
    let mut joins: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let commands = Arc::new(build_command_registry(shared.goat_root.clone(), raw.id));
    let command_specs = commands.specs();

    for binding in &raw.bindings {
        let Some(channel) = shared.channels.get(binding.name.as_str()) else {
            warn!(
                persona = %raw.slug,
                binding = %binding.name,
                "skipping binding: no compiled-in channel/plugin with this name",
            );
            continue;
        };
        let chan_binding = ChannelBinding {
            instance: InstanceId::new(),
            config: binding.config.clone(),
            commands: command_specs.clone(),
        };
        match channel.clone().bind(raw.id, chan_binding).await {
            Ok((handle, mut rx)) => {
                let bus_for_pump = shared.bus.clone();
                joins.push(tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        bus_for_pump.publish(Event::Incoming(msg));
                    }
                }));
                handles.push(handle);
            }
            Err(e) => warn!(
                persona = %raw.slug,
                binding = %binding.name,
                error = ?e,
                "skipping binding: bind failed",
            ),
        }
    }

    if handles.is_empty() {
        anyhow::bail!("no successful channel bindings");
    }

    let brain = Arc::new(Brain::new(
        raw.id,
        Arc::new(raw.personality.clone()),
        raw.default_model.clone(),
        raw.history_window,
        shared.providers.clone(),
        shared.tools.clone(),
        commands,
        shared.store.clone(),
        shared.renderer.clone(),
        shared.goat_root.clone(),
    ));
    let bus = shared.bus.clone();
    joins.push(tokio::spawn(async move {
        if let Err(e) = brain.run(bus, handles).await {
            warn!(error = ?e, "brain exited");
        }
    }));

    Ok(joins)
}

fn build_command_registry(
    goat_root: std::path::PathBuf,
    persona: goat_types::PersonaId,
) -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    for factory in inventory::iter::<CommandFactory>() {
        (factory.register)(
            &mut registry,
            CommandProviderContext::new(goat_root.clone(), persona),
        );
        info!(
            provider = factory.id,
            commands = registry.specs().len(),
            "loaded command provider"
        );
    }
    registry
}

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use goat_brain::{Brain, ProviderRegistry};
use goat_bus::EventBus;
use goat_channel::{Channel, ChannelBinding, ChannelFactory, ChannelHandle};
use goat_command::{CommandFactory, CommandProviderContext, CommandRegistry};
use goat_config::{GoatPaths, LoadedConfig};
use goat_credentials::JsonFileStore;
use goat_evaluator::{Evaluator, ModelScoreStore, NoopEvaluator};
use goat_llm::{CredentialStore, EmbeddingProvider, EmbeddingProviderSpec, LlmProviderSpec};
use goat_memory::{Embedder, MemoryStore, SqliteMemory};
use goat_persona::PersonaConfig;
use goat_render::{DefaultStreamRenderer, StreamRenderer};
use goat_store::{SqliteStore, Store};
use goat_tool::ToolRegistry;
use goat_types::{Event, InstanceId, PersonaId};
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

        let sqlite_store = SqliteStore::open(&cfg.paths.state_db)
            .await
            .context("open store")?;
        // Memory tables carry FKs into personas/conversations and therefore
        // share goat.db; reuse the store's pool rather than opening a second
        // database file. The evaluator's model-score table lives in the same
        // pool for the same reason.
        let pool = sqlite_store.pool();
        let memory: Arc<dyn MemoryStore> = Arc::new(SqliteMemory::from_pool(pool.clone()));
        let evaluator: Arc<dyn Evaluator> = Arc::new(NoopEvaluator);
        let model_scores = Arc::new(ModelScoreStore::new(pool));
        let store: Arc<dyn Store> = Arc::new(sqlite_store);

        let credentials: Arc<dyn CredentialStore> = Arc::new(
            JsonFileStore::open(cfg.paths.credentials_json.clone())
                .context("opening credentials store")?,
        );
        let providers = build_provider_registry(credentials.clone());
        let embedding_providers = build_embedding_providers(credentials);
        let embedders = build_embedders(&cfg.personas, &embedding_providers).await;
        let channels = build_channel_registry();

        let bus = EventBus::new();
        let (scheduler_handle, prepared_scheduler) =
            goat_loop::scheduler::prepare_scheduler(store.clone(), bus.clone())
                .await
                .context("prepare scheduler")?;

        let mut tools_reg = ToolRegistry::from_inventory();
        goat_tool_schedule::register(&mut tools_reg, store.clone(), scheduler_handle);
        // Per-persona recall depth so the `recall` tool honours each persona's
        // configured `episodic_k`, matching the brain's own episodic recall.
        let recall_k: Arc<HashMap<PersonaId, usize>> = Arc::new(
            cfg.personas
                .iter()
                .filter(|p| p.memory.enabled)
                .map(|p| (p.id, p.memory.episodic_k))
                .collect(),
        );
        goat_tool_memory::register(&mut tools_reg, memory.clone(), embedders.clone(), recall_k);
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
            memory,
            embedders,
            renderer,
            evaluator,
            model_scores,
            bus,
        };

        for raw_persona in &cfg.personas {
            match spawn_persona(raw_persona, &shared).await {
                Ok(handles) => join_handles.extend(handles),
                Err(e) => warn!(persona = %raw_persona.slug, error = ?e, "skipping persona"),
            }
        }

        // Yield once so every spawned brain task gets polled to its
        // first `await` (i.e. has subscribed to the bus) before the
        // scheduler is allowed to publish anything.
        tokio::task::yield_now().await;
        join_handles.push(prepared_scheduler.spawn());

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

fn build_provider_registry(credentials: Arc<dyn CredentialStore>) -> Arc<ProviderRegistry> {
    let mut reg = ProviderRegistry::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for spec in inventory::iter::<LlmProviderSpec>() {
        if !seen.insert(spec.id.as_str().to_string()) {
            warn!(
                provider = spec.id.as_str(),
                "duplicate LLM provider ID in inventory; first registration wins",
            );
            continue;
        }
        let provider = (spec.build)(credentials.clone());
        info!(provider = spec.id.as_str(), "loaded provider");
        reg.insert(provider);
    }
    Arc::new(reg)
}

fn build_embedding_providers(
    credentials: Arc<dyn CredentialStore>,
) -> HashMap<String, Arc<dyn EmbeddingProvider>> {
    let mut map: HashMap<String, Arc<dyn EmbeddingProvider>> = HashMap::new();
    for spec in inventory::iter::<EmbeddingProviderSpec>() {
        let id = spec.id.as_str().to_string();
        if map.contains_key(&id) {
            warn!(
                provider = %id,
                "duplicate embedding provider ID in inventory; first registration wins",
            );
            continue;
        }
        map.insert(id, (spec.build)(credentials.clone()));
    }
    map
}

/// Resolve each memory-enabled persona's embedder. A probe call determines
/// the embedding dimension and validates credentials up front; on failure
/// the persona simply runs with core-only memory (no episodic capture or
/// recall) rather than failing to boot.
async fn build_embedders(
    personas: &[PersonaConfig],
    providers: &HashMap<String, Arc<dyn EmbeddingProvider>>,
) -> Arc<HashMap<PersonaId, Arc<dyn Embedder>>> {
    let mut map: HashMap<PersonaId, Arc<dyn Embedder>> = HashMap::new();
    for persona in personas {
        if !persona.memory.enabled {
            continue;
        }
        let Some(settings) = persona.memory.embedding.as_ref() else {
            continue;
        };
        let Some(provider) = providers.get(&settings.provider) else {
            warn!(
                persona = %persona.slug,
                provider = %settings.provider,
                "memory: unknown embedding provider; episodic memory disabled for this persona",
            );
            continue;
        };
        match provider.embed(&settings.model, "dimension probe").await {
            Ok(probe) => {
                info!(
                    persona = %persona.slug,
                    provider = %settings.provider,
                    model = %settings.model,
                    dim = probe.len(),
                    "memory: embedder ready",
                );
                map.insert(
                    persona.id,
                    Arc::new(ProviderEmbedder {
                        provider: provider.clone(),
                        model: settings.model.clone(),
                        dim: probe.len(),
                    }),
                );
            }
            Err(e) => warn!(
                persona = %persona.slug,
                provider = %settings.provider,
                model = %settings.model,
                error = ?e,
                "memory: embedding probe failed; episodic memory disabled for this persona",
            ),
        }
    }
    Arc::new(map)
}

/// Adapter bridging a [`EmbeddingProvider`] (which is provider-aware) to the
/// provider-agnostic [`Embedder`] trait that `goat-memory`/`goat-brain`
/// consume. Lives in the wiring layer so the shared crates stay free of
/// concrete provider knowledge.
struct ProviderEmbedder {
    provider: Arc<dyn EmbeddingProvider>,
    model: String,
    dim: usize,
}

#[async_trait::async_trait]
impl Embedder for ProviderEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.provider
            .embed(&self.model, text)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
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
    memory: Arc<dyn MemoryStore>,
    embedders: Arc<HashMap<PersonaId, Arc<dyn Embedder>>>,
    renderer: Arc<dyn StreamRenderer>,
    evaluator: Arc<dyn Evaluator>,
    model_scores: Arc<ModelScoreStore>,
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
        .tools
        .validate_default_selectors(&raw.tool_selectors)
        .with_context(|| format!("invalid tools for persona {}", raw.slug))?;

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
        let instance_slug = format!("{}/{}/{}", raw.id, channel.id(), binding.name);
        let chan_binding = ChannelBinding {
            instance: InstanceId::from_slug(&instance_slug),
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
        raw.tool_selectors.clone(),
        shared.providers.clone(),
        shared.tools.clone(),
        commands,
        shared.store.clone(),
        shared.memory.clone(),
        shared.embedders.get(&raw.id).cloned(),
        raw.memory.enabled,
        raw.memory.episodic_k,
        raw.memory.summarize,
        shared.renderer.clone(),
        shared.evaluator.clone(),
        shared.model_scores.clone(),
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

#[cfg(test)]
mod tests {
    //! Boot-sequence integration tests. The concrete provider/channel crates
    //! are only linked by the final binary, so in this unit crate the inventory
    //! registries are empty. That is exactly what makes these tests valuable:
    //! they exercise the graceful-degradation paths (unknown provider, missing
    //! embedder, unbindable channel) and prove boot still succeeds, spinning up
    //! only the scheduler, rather than panicking or aborting.
    use super::*;
    use goat_llm::{Model, ProviderId};
    use goat_persona::{EmbeddingSettings, MemoryConfig, PersonaConfig, PersonalityCard};

    fn paths_in(dir: &Path) -> GoatPaths {
        GoatPaths {
            root: dir.to_path_buf(),
            credentials_json: dir.join("credentials.json"),
            personas_dir: dir.join("personas"),
            skills_dir: dir.join("skills"),
            state_db: dir.join("goat.db"),
            logs_dir: dir.join("logs"),
        }
    }

    fn persona(slug: &str, model: &str) -> PersonaConfig {
        PersonaConfig {
            id: PersonaId::from_slug(slug),
            slug: slug.into(),
            display: slug.into(),
            personality: PersonalityCard {
                system_prompt: "you are a test persona".into(),
                traits: vec![],
                source_path: Default::default(),
            },
            default_model: Model::new(ProviderId::new("openai"), model),
            history_window: 10,
            tool_selectors: vec![],
            bindings: vec![],
            memory: MemoryConfig::default(),
        }
    }

    #[tokio::test]
    async fn boots_with_no_personas_and_spawns_only_scheduler() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            personas: vec![],
        };
        let goat = Goat::boot_inner(cfg, None).await.expect("boot");
        assert_eq!(
            goat.join_handles.len(),
            1,
            "expected only the scheduler task"
        );
    }

    #[tokio::test]
    async fn persona_with_unresolvable_provider_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LoadedConfig {
            paths: paths_in(dir.path()),
            personas: vec![persona("alice", "openai/gpt-5.1")],
        };
        // The model's provider can't be resolved (no provider crates linked),
        // so the persona is skipped — but boot must still succeed.
        let goat = Goat::boot_inner(cfg, None).await.expect("boot");
        assert_eq!(goat.join_handles.len(), 1);
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
            personas: vec![p],
        };
        // Embedding provider isn't linked, so the probe path is skipped and the
        // persona would run core-only. Boot must not fail.
        let goat = Goat::boot_inner(cfg, None).await.expect("boot");
        assert_eq!(goat.join_handles.len(), 1);
    }
}

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use goat_llm::Model;
use goat_persona::{
    AutonomyConfig, EmbeddingSettings, MemoryConfig, PersonaBinding, PersonaConfig, PersonalityCard,
};
use goat_types::PersonaId;
use serde::Deserialize;
use thiserror::Error;
use tracing::warn;

const DEFAULT_HISTORY_WINDOW: usize = 20;
const DEFAULT_EPISODIC_K: usize = 5;
const LEGACY_STATE_DB: &str = "state.db";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid model in persona '{slug}': {source}")]
    Model {
        slug: String,
        #[source]
        source: goat_llm::ModelError,
    },
    #[error("persona '{slug}' has no persona.md")]
    MissingPersona { slug: String },
    #[error("personas dir not found: {0}")]
    MissingPersonasDir(PathBuf),
}

#[derive(Clone, Debug)]
pub struct GoatPaths {
    pub root: PathBuf,
    pub credentials_json: PathBuf,
    pub personas_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub state_db: PathBuf,
    pub logs_dir: PathBuf,
}

impl GoatPaths {
    pub fn default_layout() -> Result<Self> {
        Ok(Self::from_root(home_root()?))
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            credentials_json: root.join("credentials.json"),
            personas_dir: root.join("personas"),
            skills_dir: root.join("skills"),
            state_db: root.join("goat.db"),
            logs_dir: root.join("logs"),
            root,
        }
    }
}

fn home_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("$HOME is not set"))?;
    Ok(PathBuf::from(home).join(".goat"))
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub paths: GoatPaths,
    pub personas: Vec<PersonaConfig>,
}

pub async fn load() -> Result<LoadedConfig> {
    load_from(GoatPaths::default_layout()?).await
}

pub async fn load_from(paths: GoatPaths) -> Result<LoadedConfig> {
    fs::create_dir_all(&paths.root).ok();
    fs::create_dir_all(&paths.logs_dir).ok();
    fs::create_dir_all(&paths.personas_dir).ok();
    fs::create_dir_all(&paths.skills_dir).ok();

    migrate_legacy_db(&paths);

    let personas = scan_personas(&paths.personas_dir).await?;

    Ok(LoadedConfig { paths, personas })
}

fn migrate_legacy_db(paths: &GoatPaths) {
    let legacy = paths.root.join(LEGACY_STATE_DB);
    if legacy.exists() && !paths.state_db.exists() {
        match fs::rename(&legacy, &paths.state_db) {
            Ok(_) => warn!(
                from = %legacy.display(),
                to = %paths.state_db.display(),
                "renamed legacy database",
            ),
            Err(e) => warn!(
                error = ?e,
                "failed to rename legacy state.db; v0 history will not be visible",
            ),
        }
    }
}

async fn scan_personas(dir: &Path) -> Result<Vec<PersonaConfig>> {
    if !dir.exists() {
        return Err(ConfigError::MissingPersonasDir(dir.to_path_buf()).into());
    }
    let mut personas = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let slug = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) if !s.starts_with('.') => s.to_string(),
            _ => continue,
        };
        match load_persona(&path, &slug) {
            Ok(p) => personas.push(p),
            Err(e) => warn!(persona = %slug, error = ?e, "skipping persona"),
        }
    }
    Ok(personas)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersonaRuntimeConfig {
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    channels: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    history_window: Option<usize>,
    #[serde(default)]
    memory: Option<MemoryRuntimeConfig>,
    #[serde(default)]
    autonomy: Option<AutonomyRuntimeConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryRuntimeConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    embedding: Option<EmbeddingRuntimeConfig>,
    #[serde(default)]
    recall: Option<RecallRuntimeConfig>,
    #[serde(default)]
    summarization: Option<SummarizationRuntimeConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SummarizationRuntimeConfig {
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AutonomyRuntimeConfig {
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmbeddingRuntimeConfig {
    provider: String,
    model: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecallRuntimeConfig {
    #[serde(default)]
    episodic_k: Option<usize>,
}

impl MemoryRuntimeConfig {
    fn into_config(self) -> MemoryConfig {
        let episodic_k = self
            .recall
            .and_then(|r| r.episodic_k)
            .unwrap_or(DEFAULT_EPISODIC_K);
        MemoryConfig {
            enabled: self.enabled,
            embedding: self.embedding.map(|e| EmbeddingSettings {
                provider: e.provider,
                model: e.model,
            }),
            episodic_k,
            summarize: self.summarization.map(|s| s.enabled).unwrap_or(false),
        }
    }
}

fn load_persona(dir: &Path, slug: &str) -> Result<PersonaConfig> {
    let persona_md = dir.join("persona.md");
    if !persona_md.exists() {
        return Err(ConfigError::MissingPersona {
            slug: slug.to_string(),
        }
        .into());
    }
    let raw = fs::read_to_string(&persona_md)?;
    let runtime = load_runtime_config(dir)?;

    let model_raw = runtime
        .model
        .as_deref()
        .ok_or_else(|| anyhow!("persona '{slug}' missing model in config.json"))?;
    let model = Model::parse(model_raw).map_err(|source| ConfigError::Model {
        slug: slug.to_string(),
        source,
    })?;

    let personality = PersonalityCard {
        system_prompt: raw.trim().to_string(),
        traits: Vec::new(),
        source_path: persona_md,
    };

    let bindings = bindings_from_config(&runtime.channels);
    let memory = runtime
        .memory
        .map(MemoryRuntimeConfig::into_config)
        .unwrap_or_default();
    let autonomy = runtime
        .autonomy
        .map(|a| AutonomyConfig { enabled: a.enabled })
        .unwrap_or_default();

    Ok(PersonaConfig {
        id: PersonaId::from_slug(slug),
        slug: slug.to_string(),
        display: runtime.display.unwrap_or_else(|| slug.to_string()),
        personality,
        default_model: model,
        history_window: runtime.history_window.unwrap_or(DEFAULT_HISTORY_WINDOW),
        tool_selectors: runtime.tools.unwrap_or_else(|| vec!["*".to_string()]),
        bindings,
        memory,
        autonomy,
    })
}

fn load_runtime_config(dir: &Path) -> Result<PersonaRuntimeConfig> {
    let path = dir.join("config.json");
    if !path.exists() {
        return Err(anyhow!("missing {}", path.display()));
    }
    let raw = fs::read_to_string(&path)?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn bindings_from_config(configured: &BTreeMap<String, serde_json::Value>) -> Vec<PersonaBinding> {
    configured
        .clone()
        .into_iter()
        .map(|(name, config)| PersonaBinding { name, config })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_config_uses_only_config_json() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("dev");
        fs::create_dir_all(&persona_dir).unwrap();
        fs::write(
            persona_dir.join("persona.md"),
            "---\nmodel: ignored/model\n---\nYou are dev.\n",
        )
        .unwrap();
        fs::write(
            persona_dir.join("config.json"),
            r#"{
              "display": "Developer",
              "model": "openai/gpt-4o-mini",
              "tools": ["*", "!shell"],
              "history_window": 7,
              "channels": {
                "telegram": { "token": "new" }
              }
            }"#,
        )
        .unwrap();
        fs::write(persona_dir.join("stray.json"), r#"{ "token": "ignored" }"#).unwrap();

        let p = load_persona(&persona_dir, "dev").unwrap();
        assert!(p.personality.system_prompt.contains("ignored/model"));
        assert_eq!(p.display, "Developer");
        assert_eq!(p.history_window, 7);
        assert_eq!(p.tool_selectors, vec!["*", "!shell"]);
        assert_eq!(p.bindings.len(), 1);
        assert_eq!(p.bindings[0].name, "telegram");
        assert_eq!(p.bindings[0].config["token"], "new");
        assert!(!p.memory.enabled, "memory defaults to disabled");
    }

    #[test]
    fn memory_section_parses() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("dev");
        fs::create_dir_all(&persona_dir).unwrap();
        fs::write(persona_dir.join("persona.md"), "You are dev.\n").unwrap();
        fs::write(
            persona_dir.join("config.json"),
            r#"{
              "model": "anthropic/claude-x",
              "channels": {},
              "memory": {
                "enabled": true,
                "embedding": { "provider": "openai", "model": "text-embedding-3-small" },
                "recall": { "episodic_k": 8 }
              }
            }"#,
        )
        .unwrap();

        let p = load_persona(&persona_dir, "dev").unwrap();
        assert!(p.memory.enabled);
        assert_eq!(p.memory.episodic_k, 8);
        assert!(
            !p.memory.summarize,
            "summarization defaults off when absent"
        );
        let emb = p.memory.embedding.expect("embedding configured");
        assert_eq!(emb.provider, "openai");
        assert_eq!(emb.model, "text-embedding-3-small");
    }

    #[test]
    fn summarization_flag_parses() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("dev");
        fs::create_dir_all(&persona_dir).unwrap();
        fs::write(persona_dir.join("persona.md"), "You are dev.\n").unwrap();
        fs::write(
            persona_dir.join("config.json"),
            r#"{
              "model": "anthropic/claude-x",
              "channels": {},
              "memory": { "summarization": { "enabled": true } }
            }"#,
        )
        .unwrap();

        let p = load_persona(&persona_dir, "dev").unwrap();
        assert!(p.memory.summarize);
        assert!(!p.memory.enabled, "summarization is independent of enabled");
    }

    #[test]
    fn memory_defaults_episodic_k_when_recall_absent() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("dev");
        fs::create_dir_all(&persona_dir).unwrap();
        fs::write(persona_dir.join("persona.md"), "You are dev.\n").unwrap();
        fs::write(
            persona_dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "channels": {}, "memory": { "enabled": true } }"#,
        )
        .unwrap();

        let p = load_persona(&persona_dir, "dev").unwrap();
        assert!(p.memory.enabled);
        assert_eq!(p.memory.episodic_k, DEFAULT_EPISODIC_K);
        assert!(p.memory.embedding.is_none());
    }

    #[test]
    fn autonomy_flag_parses() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("dev");
        fs::create_dir_all(&persona_dir).unwrap();
        fs::write(persona_dir.join("persona.md"), "You are dev.\n").unwrap();
        fs::write(
            persona_dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "channels": {}, "autonomy": { "enabled": true } }"#,
        )
        .unwrap();

        let p = load_persona(&persona_dir, "dev").unwrap();
        assert!(p.autonomy.enabled);
    }

    #[test]
    fn autonomy_defaults_off_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("dev");
        fs::create_dir_all(&persona_dir).unwrap();
        fs::write(persona_dir.join("persona.md"), "You are dev.\n").unwrap();
        fs::write(
            persona_dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "channels": {} }"#,
        )
        .unwrap();

        let p = load_persona(&persona_dir, "dev").unwrap();
        assert!(!p.autonomy.enabled, "autonomy defaults to disabled");
    }
}

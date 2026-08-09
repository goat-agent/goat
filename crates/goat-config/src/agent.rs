use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use goat_agent_config::{
    AgentBinding, AgentCard, AgentConfig, AgentIntegration, AutonomyConfig, EmbeddingSettings,
    MemoryConfig, WatchSourceEntry, WatchWorkflow,
};
use goat_model::Model;
use goat_types::AgentId;
use serde::Deserialize;
use tracing::warn;

use crate::ConfigError;

const DEFAULT_HISTORY_WINDOW: usize = 20;
const DEFAULT_EPISODIC_K: usize = 5;
const DEFAULT_INTAKE_DEBOUNCE_MS: u64 = 1000;
const DEFAULT_INTAKE_CEILING_MS: u64 = 5000;

pub const AGENT_DEFINITION_FILE: &str = "agent.md";

pub(crate) fn scan_agents(dir: &Path) -> Result<Vec<AgentConfig>> {
    if !dir.exists() {
        return Err(ConfigError::MissingAgentsDir(dir.to_path_buf()).into());
    }
    let mut agents = Vec::new();
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
        match load_agent(&path, &slug) {
            Ok(p) => agents.push(p),
            Err(e) => warn!(agent = %slug, error = ?e, "skipping agent"),
        }
    }
    Ok(agents)
}

fn load_agent(dir: &Path, slug: &str) -> Result<AgentConfig> {
    let definition = dir.join(AGENT_DEFINITION_FILE);
    if !definition.exists() {
        return Err(ConfigError::MissingDefinition {
            slug: slug.to_string(),
        }
        .into());
    }
    let raw = fs::read_to_string(&definition)?;
    let runtime = load_runtime_config(dir)?;

    let model_raw = runtime
        .model
        .as_deref()
        .ok_or_else(|| anyhow!("agent '{slug}' missing model in config.json"))?;
    let model = Model::parse(model_raw).map_err(|source| ConfigError::Model {
        slug: slug.to_string(),
        source,
    })?;
    let timezone = runtime
        .timezone
        .map(|value| validate_timezone(slug, value))
        .transpose()?;

    let personality = AgentCard {
        system_prompt: raw.trim().to_string(),
        source_path: definition,
    };

    let bindings = bindings_from_config(&runtime.channels);
    let integrations = integrations_from_config(&runtime.integrations);
    let watch = runtime.watch.map(watch_from_config);
    let memory = runtime
        .memory
        .map(MemoryRuntimeConfig::into_config)
        .unwrap_or_default();
    let autonomy = runtime
        .autonomy
        .map(|a| AutonomyConfig { enabled: a.enabled })
        .unwrap_or_default();

    Ok(AgentConfig {
        id: AgentId::from_slug(slug),
        slug: slug.to_string(),
        display: runtime.display.unwrap_or_else(|| slug.to_string()),
        personality,
        default_model: model,
        timezone,
        history_window: runtime.history_window.unwrap_or(DEFAULT_HISTORY_WINDOW),
        tool_selectors: runtime.tools.unwrap_or_else(|| vec!["*".to_string()]),
        bindings,
        integrations,
        watch,
        memory,
        autonomy,
        intake_debounce: std::time::Duration::from_millis(
            runtime
                .intake_debounce_ms
                .unwrap_or(DEFAULT_INTAKE_DEBOUNCE_MS),
        ),
        intake_ceiling: std::time::Duration::from_millis(
            runtime
                .intake_ceiling_ms
                .unwrap_or(DEFAULT_INTAKE_CEILING_MS),
        ),
    })
}

fn validate_timezone(slug: &str, value: String) -> Result<String> {
    value
        .parse::<chrono_tz::Tz>()
        .map(|timezone| timezone.to_string())
        .map_err(|_| {
            ConfigError::Timezone {
                slug: slug.to_string(),
                value,
            }
            .into()
        })
}

fn load_runtime_config(dir: &Path) -> Result<AgentRuntimeConfig> {
    let path = dir.join("config.json");
    if !path.exists() {
        return Err(anyhow!("missing {}", path.display()));
    }
    let raw = fs::read_to_string(&path)?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn bindings_from_config(configured: &BTreeMap<String, serde_json::Value>) -> Vec<AgentBinding> {
    configured
        .clone()
        .into_iter()
        .map(|(name, config)| AgentBinding { name, config })
        .collect()
}

fn integrations_from_config(
    configured: &BTreeMap<String, serde_json::Value>,
) -> Vec<AgentIntegration> {
    configured
        .clone()
        .into_iter()
        .map(|(name, config)| AgentIntegration { name, config })
        .collect()
}

fn watch_from_config(
    configured: BTreeMap<String, Vec<WatchEntryRuntimeConfig>>,
) -> Vec<WatchWorkflow> {
    configured
        .into_iter()
        .map(|(name, sources)| WatchWorkflow {
            name,
            sources: sources
                .into_iter()
                .map(|entry| WatchSourceEntry {
                    source: entry.source,
                    query: entry.query,
                    id: entry.id,
                    stream: entry.stream,
                })
                .collect(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchEntryRuntimeConfig {
    source: String,
    query: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    stream: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRuntimeConfig {
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    channels: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    integrations: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    watch: Option<BTreeMap<String, Vec<WatchEntryRuntimeConfig>>>,
    #[serde(default)]
    history_window: Option<usize>,
    #[serde(default)]
    memory: Option<MemoryRuntimeConfig>,
    #[serde(default)]
    autonomy: Option<AutonomyRuntimeConfig>,
    #[serde(default)]
    intake_debounce_ms: Option<u64>,
    #[serde(default)]
    intake_ceiling_ms: Option<u64>,
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
            summarize: self.summarization.is_some_and(|s| s.enabled),
        }
    }
}

pub const AGENT_CONFIG_FILE: &str = "config.toml";

pub struct AgentDocument {
    path: std::path::PathBuf,
    document: toml_edit::DocumentMut,
    present: bool,
}

impl AgentDocument {
    #[must_use]
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(AGENT_CONFIG_FILE);
        let present = path.exists() || dir.join("config.json").exists();
        Self {
            document: crate::document::read_document(&path),
            path,
            present,
        }
    }

    #[must_use]
    pub fn create(dir: &Path, display: &str, model: &str) -> Self {
        let mut document = toml_edit::DocumentMut::new();
        document["display"] = toml_edit::value(display);
        document["model"] = toml_edit::value(model);
        document["tools"] = toml_edit::value(toml_edit::Array::from_iter(["*"]));
        Self {
            path: dir.join(AGENT_CONFIG_FILE),
            document,
            present: true,
        }
    }

    #[must_use]
    pub fn present(&self) -> bool {
        self.present
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn field(&self, key: &str) -> Option<&str> {
        self.document.get(key).and_then(toml_edit::Item::as_str)
    }

    #[must_use]
    pub fn render(&self) -> String {
        self.document.to_string()
    }

    #[must_use]
    pub fn section_entries(&self, section: &str) -> Vec<(String, serde_json::Value)> {
        self.document
            .get(section)
            .and_then(toml_edit::Item::as_table)
            .map(|table| {
                table
                    .iter()
                    .map(|(key, item)| (key.to_owned(), crate::value::to_json(item)))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[must_use]
    pub fn section_keys(&self, section: &str) -> Vec<String> {
        self.section_entries(section)
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    pub fn upsert_section(&mut self, section: &str, key: &str, value: &serde_json::Value) {
        let Some(incoming) = crate::value::from_json(value) else {
            return;
        };
        let table = crate::document::container(&mut self.document, &[section]);
        match (table.get_mut(key), &incoming) {
            (Some(existing), toml_edit::Item::Table(new)) if existing.is_table() => {
                let existing = existing.as_table_mut().expect("checked just above");
                for (key, item) in new {
                    existing.insert(key, item.clone());
                }
            }
            _ => {
                table.insert(key, incoming);
            }
        }
    }

    pub fn remove_section(&mut self, section: &str, key: &str) {
        crate::document::container(&mut self.document, &[section]).remove(key);
    }

    pub fn remove_slots(&mut self, section: &str, key: &str, slots: &[&str]) {
        let table = crate::document::section(&mut self.document, &[section, key]);
        for slot in slots {
            table.remove(slot);
        }
    }

    pub fn save(&self) -> Result<(), crate::SettingsError> {
        crate::write_atomic(&self.path, self.document.to_string().as_bytes())
    }
}

#[cfg(test)]
mod document_tests {
    use super::AgentDocument;
    use serde_json::json;

    #[test]
    fn a_created_agent_holds_only_what_it_was_given() {
        let directory = tempfile::tempdir().unwrap();
        let document = AgentDocument::create(directory.path(), "main", "anthropic/claude-x");
        let rendered = document.render();
        assert!(rendered.contains("display = \"main\""));
        assert!(rendered.contains("model = \"anthropic/claude-x\""));
        assert!(rendered.contains("tools = [\"*\"]"));
        assert_eq!(document.field("model"), Some("anthropic/claude-x"));
    }

    #[test]
    fn a_binding_with_no_keys_is_still_written() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = AgentDocument::create(directory.path(), "main", "anthropic/claude-x");
        document.upsert_section("channels", "discord", &json!({}));
        document.save().unwrap();

        let saved = document.render();
        assert!(saved.contains("[channels.discord]"));
        assert!(!saved.contains("\n[channels]"));
        assert_eq!(document.section_keys("channels"), vec!["discord"]);
    }

    #[test]
    fn upserting_over_a_binding_merges_rather_than_replaces() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = AgentDocument::create(directory.path(), "main", "anthropic/claude-x");
        document.upsert_section("integrations", "linear", &json!({ "account": "default" }));
        document.upsert_section("integrations", "linear", &json!({ "user_id": "u1" }));

        let entries = document.section_entries("integrations");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].1,
            json!({ "account": "default", "user_id": "u1" })
        );
    }

    #[test]
    fn evicting_secret_slots_leaves_the_binding_behind() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = AgentDocument::create(directory.path(), "main", "anthropic/claude-x");
        document.upsert_section(
            "channels",
            "slack",
            &json!({ "bot_token": "xoxb", "app_token": "xapp", "team": "acme" }),
        );
        document.remove_slots("channels", "slack", &["bot_token", "app_token"]);

        let entries = document.section_entries("channels");
        assert_eq!(entries[0].1, json!({ "team": "acme" }));
    }

    #[test]
    fn a_legacy_agent_config_migrates_on_load() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("config.json"),
            r#"{"model":"anthropic/claude-x","channels":{"discord":{}},
                "watch":{"inbox":[{"source":"linear","query":"is:open"}]}}"#,
        )
        .unwrap();

        let document = AgentDocument::load(directory.path());
        assert!(document.present());
        assert_eq!(document.field("model"), Some("anthropic/claude-x"));
        assert_eq!(document.section_keys("channels"), vec!["discord"]);

        let rendered = document.render();
        assert!(rendered.contains("[channels.discord]"));
        assert!(rendered.contains("[[watch.inbox]]"));
        assert!(directory.path().join("config.toml").exists());
        assert!(directory.path().join("config.json.migrated").exists());
    }

    #[test]
    fn a_missing_agent_config_is_reported_not_invented() {
        let directory = tempfile::tempdir().unwrap();
        assert!(!AgentDocument::load(directory.path()).present());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_agent_from_agent_md() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents").join("main");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agent.md"), "You are main.").unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "channels": {} }"#,
        )
        .unwrap();

        let agents = scan_agents(&tmp.path().join("agents")).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].slug, "main");
        assert_eq!(agents[0].personality.system_prompt, "You are main.");
        assert_eq!(agents[0].timezone, None);
        assert!(agents[0].integrations.is_empty());
    }

    #[test]
    fn loads_canonical_owner_timezone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents").join("main");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agent.md"), "You are main.").unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "timezone": "Asia/Seoul" }"#,
        )
        .unwrap();

        let agent = load_agent(&dir, "main").unwrap();
        assert_eq!(agent.timezone.as_deref(), Some("Asia/Seoul"));
    }

    #[test]
    fn invalid_owner_timezone_names_agent_and_value() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents").join("main");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agent.md"), "You are main.").unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "timezone": "Korea/Typo" }"#,
        )
        .unwrap();

        let error = load_agent(&dir, "main").unwrap_err().to_string();
        assert_eq!(
            error,
            "invalid timezone in agent 'main': 'Korea/Typo' is not a canonical IANA timezone"
        );
    }

    #[test]
    fn loads_agent_integrations() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents").join("main");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agent.md"), "You are main.").unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "channels": {},
                 "integrations": { "linear": { "account": "default" } } }"#,
        )
        .unwrap();

        let agents = scan_agents(&tmp.path().join("agents")).unwrap();
        assert_eq!(agents[0].integrations.len(), 1);
        assert_eq!(agents[0].integrations[0].name, "linear");
        assert_eq!(agents[0].integrations[0].config["account"], "default");
        assert!(agents[0].watch.is_none());
    }

    #[test]
    fn loads_the_watch_section_as_named_workflows() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents").join("main");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agent.md"), "You are main.").unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x",
                 "watch": { "inbox": [
                   { "source": "linear", "query": "assignee:@me is:open" },
                   { "source": "github", "query": "is:open assignee:@me", "id": "github-assigned", "stream": "assigned" }
                 ] } }"#,
        )
        .unwrap();

        let agents = scan_agents(&tmp.path().join("agents")).unwrap();
        let watch = agents[0].watch.as_ref().unwrap();
        assert_eq!(watch.len(), 1);
        assert_eq!(watch[0].name, "inbox");
        assert_eq!(watch[0].sources.len(), 2);
        assert_eq!(watch[0].sources[0].source, "linear");
        assert_eq!(watch[0].sources[0].id, None);
        assert_eq!(watch[0].sources[0].stream, None);
        assert_eq!(watch[0].sources[1].id.as_deref(), Some("github-assigned"));
        assert_eq!(watch[0].sources[1].stream.as_deref(), Some("assigned"));
    }

    #[test]
    fn an_empty_watch_section_disables_defaults_and_typos_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("agents").join("main");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agent.md"), "You are main.").unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x", "watch": {} }"#,
        )
        .unwrap();
        let agents = scan_agents(&tmp.path().join("agents")).unwrap();
        assert_eq!(agents[0].watch.as_ref().unwrap().len(), 0);

        fs::write(
            dir.join("config.json"),
            r#"{ "model": "anthropic/claude-x",
                 "watch": { "inbox": [ { "source": "linear", "querry": "x" } ] } }"#,
        )
        .unwrap();
        assert!(scan_agents(&tmp.path().join("agents")).unwrap().is_empty());
    }
}

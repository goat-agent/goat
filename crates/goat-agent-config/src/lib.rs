use std::path::PathBuf;

use goat_model::Model;
use goat_types::AgentId;

#[derive(Clone, Debug)]
pub struct AgentCard {
    pub system_prompt: String,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct AgentBinding {
    pub name: String,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct AgentIntegration {
    pub name: String,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub id: AgentId,
    pub slug: String,
    pub display: String,
    pub personality: AgentCard,
    pub default_model: Model,
    pub history_window: usize,
    pub tool_selectors: Vec<String>,
    pub bindings: Vec<AgentBinding>,
    pub integrations: Vec<AgentIntegration>,
    pub memory: MemoryConfig,
    pub autonomy: AutonomyConfig,
    pub intake_debounce: std::time::Duration,
    pub intake_ceiling: std::time::Duration,
}

#[derive(Clone, Debug, Default)]
pub struct AutonomyConfig {
    pub enabled: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub embedding: Option<EmbeddingSettings>,
    pub episodic_k: usize,
    pub summarize: bool,
}

#[derive(Clone, Debug)]
pub struct EmbeddingSettings {
    pub provider: String,
    pub model: String,
}

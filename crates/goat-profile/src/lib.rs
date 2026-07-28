use std::path::PathBuf;

use goat_model::Model;
use goat_types::ProfileId;

#[derive(Clone, Debug)]
pub struct ProfileCard {
    pub system_prompt: String,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ProfileBinding {
    pub name: String,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ProfileIntegration {
    pub name: String,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ProfileConfig {
    pub id: ProfileId,
    pub slug: String,
    pub display: String,
    pub personality: ProfileCard,
    pub default_model: Model,
    pub history_window: usize,
    pub tool_selectors: Vec<String>,
    pub bindings: Vec<ProfileBinding>,
    pub integrations: Vec<ProfileIntegration>,
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

use std::path::PathBuf;

use goat_llm::Model;
use goat_types::PersonaId;

#[derive(Clone, Debug)]
pub struct PersonalityCard {
    pub system_prompt: String,
    pub traits: Vec<String>,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PersonaBinding {
    pub name: String,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct PersonaConfig {
    pub id: PersonaId,
    pub slug: String,
    pub display: String,
    pub personality: PersonalityCard,
    pub default_model: Model,
    pub history_window: usize,
    pub tool_selectors: Vec<String>,
    pub bindings: Vec<PersonaBinding>,
}

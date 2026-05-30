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
    pub memory: MemoryConfig,
}

/// Long-term memory settings for a persona. When `enabled` is false the
/// brain skips all memory reads and writes. Core memory always works once
/// enabled; episodic capture and recall additionally require `embedding`.
#[derive(Clone, Debug, Default)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub embedding: Option<EmbeddingSettings>,
    pub episodic_k: usize,
    /// Roll older conversation history into an LLM-maintained summary instead
    /// of dropping it past `history_window`. Independent of `enabled`.
    pub summarize: bool,
}

#[derive(Clone, Debug)]
pub struct EmbeddingSettings {
    /// Embedding provider id, e.g. `openai`. Need not match the chat model's
    /// provider (Anthropic personas can embed via OpenAI).
    pub provider: String,
    /// Embedding model id, e.g. `text-embedding-3-small`.
    pub model: String,
}

use goat_auth::CredentialStore;
use goat_provider::ModelListSource;
use goat_provider_openai_compat::{
    OpenAiCompatProvider, api_key, known_openai_compatible_vision_model,
};

pub const PROVIDER_ID: &str = "openrouter";
const BASE_URL: &str = "https://openrouter.ai/api/v1";
const HOST: &str = "openrouter.ai";
const ENV_VAR: &str = "OPENROUTER_API_KEY";

const CATALOG: &[&str] = &[
    "anthropic/claude-sonnet-4.6",
    "openai/gpt-5.4",
    "google/gemini-2.5-pro",
    "deepseek/deepseek-chat-v3-0324",
    "qwen/qwen3-coder",
    "moonshotai/kimi-k2.6",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("anthropic/claude-sonnet-4.6", 1_000_000),
    ("openai/gpt-5.4", 1_000_000),
    ("google/gemini-2.5", 1_000_000),
    ("deepseek/deepseek-chat-v3", 164_000),
    ("qwen/qwen3-coder", 1_000_000),
    ("moonshotai/kimi-k2.6", 262_144),
];

fn is_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("embedding")
        || id.contains("moderation")
        || id.contains("image")
        || id.contains("tts")
        || id.contains("whisper"))
}

fn is_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    known_openai_compatible_vision_model(&id)
        || id.contains("claude")
        || id.contains("gemini")
        || id.contains("grok-4")
}

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT_WINDOWS)
        .with_model_filter(is_chat_model)
        .with_vision_filter(is_vision_model)
        .with_reasoning_effort(false)
        .with_model_list_source(ModelListSource::Discover)
}

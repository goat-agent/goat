use goat_auth::CredentialStore;
use goat_provider::ModelListSource;
use goat_provider_openai_compat::{
    OpenAiCompatProvider, api_key, known_openai_compatible_vision_model,
};

pub const PROVIDER_ID: &str = "openrouter";
const BASE_URL: &str = "https://openrouter.ai/api/v1";
const HOST: &str = "openrouter.ai";
const ENV_VAR: &str = "OPENROUTER_API_KEY";

const REFERER: &str = "https://github.com/goat-agent/goat";
const TITLE: &str = "goat";

const CATALOG: &[&str] = &[
    "anthropic/claude-opus-5",
    "anthropic/claude-sonnet-5",
    "openai/gpt-5.6",
    "google/gemini-3.6-flash",
    "z-ai/glm-5.2",
    "moonshotai/kimi-k3",
    "deepseek/deepseek-v4-pro",
    "minimax/minimax-m3",
    "qwen/qwen3-coder",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("anthropic/claude-opus-5", 1_000_000),
    ("anthropic/claude-sonnet-5", 1_000_000),
    ("openai/gpt-5.6", 1_050_000),
    ("google/gemini-3.6-flash", 1_000_000),
    ("z-ai/glm-5.2", 1_000_000),
    ("moonshotai/kimi-k3", 1_000_000),
    ("deepseek/deepseek-v4", 1_000_000),
    ("minimax/minimax-m3", 1_000_000),
    ("qwen/qwen3-coder", 1_000_000),
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
        .with_extra_headers([("HTTP-Referer", REFERER), ("X-Title", TITLE)])
        .with_model_list_source(ModelListSource::Discover)
}

use goat_auth::CredentialStore;
use goat_provider_openai_compat::{OpenAiCompatProvider, api_key};

pub const PROVIDER_ID: &str = "groq";
const BASE_URL: &str = "https://api.groq.com/openai/v1";
const HOST: &str = "api.groq.com";
const ENV_VAR: &str = "GROQ_API_KEY";

const CATALOG: &[&str] = &[
    "llama-3.3-70b-versatile",
    "llama-3.1-8b-instant",
    "openai/gpt-oss-120b",
    "openai/gpt-oss-20b",
    "qwen/qwen3-32b",
    "qwen/qwen3.6-27b",
    "meta-llama/llama-4-scout-17b-16e-instruct",
    "groq/compound",
    "groq/compound-mini",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("llama-3.3", 131_072),
    ("llama-3.1", 131_072),
    ("openai/gpt-oss", 131_072),
    ("qwen/qwen3", 131_072),
    ("meta-llama/llama-4-scout", 131_072),
    ("groq/compound", 131_072),
];

fn is_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("whisper") || id.contains("tts") || id.contains("embedding"))
}

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT_WINDOWS)
        .with_model_filter(is_chat_model)
        .with_images(false)
        .with_stream_options(false)
        .with_reasoning_effort(false)
}

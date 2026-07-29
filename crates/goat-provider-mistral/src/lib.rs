use goat_auth::CredentialStore;
use goat_provider_openai_compat::{OpenAiCompatProvider, api_key, no_efforts};

pub const PROVIDER_ID: &str = "mistral";
const BASE_URL: &str = "https://api.mistral.ai/v1";
const HOST: &str = "api.mistral.ai";
const ENV_VAR: &str = "MISTRAL_API_KEY";

const CATALOG: &[&str] = &[
    "mistral-large-latest",
    "mistral-medium-latest",
    "mistral-small-latest",
    "ministral-3-14b-latest",
    "ministral-3-8b-latest",
    "ministral-3-3b-latest",
    "codestral-latest",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("mistral-large", 262_144),
    ("mistral-medium", 262_144),
    ("mistral-small", 262_144),
    ("ministral-3", 262_144),
    ("codestral", 262_144),
];

fn is_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("pixtral")
        || id.starts_with("mistral-large")
        || id.starts_with("mistral-medium")
        || id.starts_with("mistral-small")
}

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT_WINDOWS)
        .with_vision_filter(is_vision_model)
        .with_efforts(no_efforts)
        .with_reasoning_effort(false)
}

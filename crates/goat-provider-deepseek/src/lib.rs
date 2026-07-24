use goat_auth::CredentialStore;
use goat_provider_openai_compat::{OpenAiCompatProvider, api_key};

pub const PROVIDER_ID: &str = "deepseek";
const BASE_URL: &str = "https://api.deepseek.com";
const HOST: &str = "api.deepseek.com";
const ENV_VAR: &str = "DEEPSEEK_API_KEY";

const CATALOG: &[&str] = &[
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-chat",
    "deepseek-reasoner",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("deepseek-v4-pro", 1_000_000),
    ("deepseek-v4-flash", 1_000_000),
    ("deepseek-chat", 1_000_000),
    ("deepseek-reasoner", 1_000_000),
];

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT_WINDOWS)
        .with_images(false)
        .with_reasoning_effort(false)
}

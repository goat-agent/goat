use goat_auth::CredentialStore;
use goat_provider::ProviderMetadata;
use goat_provider_openai_compat::{
    ChatDiscovery, ChatValidation, OpenAiCompatProvider, api_key, no_efforts,
};

pub const PROVIDER_ID: &str = "kimi";

const BASE_URL: &str = "https://api.moonshot.ai/v1";
const HOST: &str = "api.moonshot.ai";
const ENV_VAR: &str = "MOONSHOT_API_KEY";

const KIMI_SETUP: &[&str] = &[
    "Kimi Platform API key provider.",
    "For Kimi Code OAuth, use `goat provider login kimi-code`.",
    "API-key setup: `goat provider login kimi --key sk-...`.",
];

const CATALOG: &[&str] = &[
    "kimi-k2.7-code",
    "kimi-k2.7-code-highspeed",
    "kimi-k2.6",
    "kimi-k2.5",
    "moonshot-v1-128k",
    "moonshot-v1-32k",
    "moonshot-v1-8k",
    "moonshot-v1-auto",
    "moonshot-v1-128k-vision-preview",
    "moonshot-v1-32k-vision-preview",
    "moonshot-v1-8k-vision-preview",
];

const CONTEXT: &[(&str, u32)] = &[
    ("kimi-k2.7", 256_000),
    ("kimi-k2.6", 256_000),
    ("kimi-k2.5", 256_000),
    ("moonshot-v1-128k-vision-preview", 128_000),
    ("moonshot-v1-32k-vision-preview", 32_000),
    ("moonshot-v1-8k-vision-preview", 8_000),
    ("moonshot-v1-128k", 128_000),
    ("moonshot-v1-32k", 32_000),
    ("moonshot-v1-8k", 8_000),
];

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT)
        .with_vision_filter(kimi_vision_model)
        .with_efforts(no_efforts)
        .with_reasoning_effort(false)
        .with_validation(ChatValidation::CatalogOnly)
        .with_discovery(ChatDiscovery::CatalogOnly)
        .with_metadata(ProviderMetadata {
            env_var: Some(ENV_VAR),
            validation: "catalog-only",
            endpoint: None,
            oauth: Some("Kimi Code OAuth is provider id kimi-code"),
            login_endpoint: None,
            setup: KIMI_SETUP,
        })
}

fn kimi_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.starts_with("kimi-k2.6") || id.contains("vision-preview")
}

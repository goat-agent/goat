use goat_auth::CredentialStore;
use goat_provider::ProviderMetadata;
use goat_provider_openai_compat::{
    ChatDiscovery, ChatValidation, OpenAiCompatProvider, api_key, no_efforts, no_vision,
};

pub const PROVIDER_ID: &str = "minimax";

const BASE_URL: &str = "https://api.minimax.io/v1";
const HOST: &str = "api.minimax.io";
const ENV_VAR: &str = "MINIMAX_API_KEY";

const SETUP: &[&str] = &[
    "MiniMax open platform API-key provider.",
    "Use `MINIMAX_API_KEY` or `goat provider login minimax --key ...`.",
    "Keys from platform.minimax.io are region-scoped; the China platform uses a different host.",
];

const CATALOG: &[&str] = &[
    "MiniMax-M3",
    "MiniMax-M2.7",
    "MiniMax-M2.7-highspeed",
    "MiniMax-M2.5",
    "MiniMax-M2.5-highspeed",
    "MiniMax-M2.1",
    "MiniMax-M2.1-highspeed",
    "MiniMax-M2",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[("MiniMax-M3", 1_000_000), ("MiniMax-M2", 204_800)];

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT_WINDOWS)
        .with_vision_filter(no_vision)
        .with_images(false)
        .with_efforts(no_efforts)
        .with_reasoning_effort(false)
        .with_validation(ChatValidation::CatalogOnly)
        .with_discovery(ChatDiscovery::CatalogOnly)
        .with_metadata(ProviderMetadata {
            env_var: Some(ENV_VAR),
            validation: "catalog-only",
            endpoint: Some(BASE_URL),
            oauth: Some("not supported"),
            login_endpoint: None,
            setup: SETUP,
        })
}

#[cfg(test)]
mod tests {
    use goat_auth::CredentialStore;
    use goat_provider::{AuthMethod, Provider};

    use super::*;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn minimax_exposes_catalog_and_windows() {
        let store = store("goat-provider-minimax.json");
        let provider = build(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::ApiKey);
        assert_eq!(provider.metadata().env_var, Some(ENV_VAR));
        assert_eq!(provider.catalog(), CATALOG);
        assert_eq!(provider.context_window("MiniMax-M3"), Some(1_000_000));
        assert_eq!(provider.context_window("MiniMax-M2.7"), Some(204_800));
        assert!(!provider.supports_images("MiniMax-M3"));
    }
}

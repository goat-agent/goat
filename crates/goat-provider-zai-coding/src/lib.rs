use goat_auth::CredentialStore;
use goat_provider::{Effort, ProviderMetadata};
use goat_provider_openai_compat::{
    ChatDiscovery, ChatValidation, OpenAiCompatProvider, api_key, no_vision,
};

pub const PROVIDER_ID: &str = "zai-coding";

const BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";
const HOST: &str = "api.z.ai";
const ENV_VAR: &str = "ZAI_CODING_API_KEY";

const ZAI_CODING_SETUP: &[&str] = &[
    "Z.AI Coding Plan API-key provider.",
    "Use `ZAI_CODING_API_KEY` or `goat provider login zai-coding --key sk-...`.",
    "This is not OAuth and does not reuse the standard `zai` credential.",
];

const CATALOG: &[&str] = &["glm-5.2", "glm-5.1", "glm-5-turbo", "glm-4.7"];

const CONTEXT: &[(&str, u32)] = &[
    ("glm-5.2", 1_000_000),
    ("glm-5.1", 198_000),
    ("glm-5-turbo", 128_000),
    ("glm-4.7", 128_000),
];

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT)
        .with_vision_filter(no_vision)
        .with_efforts(zai_efforts)
        .with_effort_wire(zai_effort_wire)
        .with_validation(ChatValidation::CatalogOnly)
        .with_discovery(ChatDiscovery::CatalogOnly)
        .with_metadata(ProviderMetadata {
            env_var: Some(ENV_VAR),
            validation: "catalog-only",
            endpoint: Some(BASE_URL),
            oauth: Some("not OAuth; uses Z.AI Coding Plan API key"),
            login_endpoint: None,
            setup: ZAI_CODING_SETUP,
        })
}

fn zai_efforts(model: &str) -> Vec<Effort> {
    if model == "glm-5.2" {
        vec![
            Effort::Off,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ]
    } else {
        Vec::new()
    }
}

fn zai_effort_wire(effort: Effort) -> Option<&'static str> {
    let wire = match effort {
        Effort::Off => "none",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    };
    (!wire.is_empty()).then_some(wire)
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
    fn zai_coding_is_distinct_api_key_provider() {
        let store = store("goat-provider-zai-coding.json");
        let provider = build(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::ApiKey);
        assert_eq!(provider.metadata().env_var, Some("ZAI_CODING_API_KEY"));
        assert_eq!(provider.metadata().endpoint, Some(BASE_URL));
        assert_eq!(provider.catalog(), CATALOG);
        assert_eq!(provider.context_window("glm-5.2"), Some(1_000_000));
    }
}

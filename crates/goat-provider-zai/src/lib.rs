use goat_auth::CredentialStore;
use goat_provider::{Effort, ProviderMetadata};
use goat_provider_openai_compat::{ChatDiscovery, ChatValidation, OpenAiCompatProvider, api_key};

pub const PROVIDER_ID: &str = "zai";

const BASE_URL: &str = "https://api.z.ai/api/paas/v4";
const HOST: &str = "api.z.ai";
const ENV_VAR: &str = "ZAI_API_KEY";

const CATALOG: &[&str] = &[
    "glm-5.2",
    "glm-5.1",
    "glm-5-turbo",
    "glm-5",
    "glm-4.7",
    "glm-4.7-flash",
    "glm-4.6",
    "glm-4.5",
    "glm-4.5-air",
    "glm-4-32b-0414-128k",
    "glm-5v-turbo",
];

const CONTEXT: &[(&str, u32)] = &[
    ("glm-5.2", 128_000),
    ("glm-5.1", 128_000),
    ("glm-5", 128_000),
    ("glm-4.7", 128_000),
    ("glm-4.6", 128_000),
    ("glm-4.5", 128_000),
    ("glm-4-32b", 128_000),
    ("glm-5v", 128_000),
];

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT)
        .with_vision_filter(zai_vision_model)
        .with_efforts(zai_efforts)
        .with_effort_wire(zai_effort_wire)
        .with_validation(ChatValidation::CatalogOnly)
        .with_discovery(ChatDiscovery::CatalogOnly)
        .with_metadata(ProviderMetadata {
            env_var: Some(ENV_VAR),
            validation: "catalog-only",
            endpoint: None,
            oauth: Some("not supported by Z.AI API docs"),
            login_endpoint: None,
            setup: &[],
        })
}

fn zai_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("glm-5v")
        || id.contains("glm-4.6v")
        || id.contains("glm-4.5v")
        || id.contains("glm-4v")
        || id.contains("vision")
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
    use goat_provider::{Effort, Provider};

    use super::*;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn metadata_is_exposed() {
        let store = store("goat-provider-zai-metadata.json");
        let provider = build(&store, "default");
        assert_eq!(provider.catalog(), CATALOG);
        assert_eq!(provider.context_window("glm-5.2"), Some(128_000));
        assert_eq!(
            provider.efforts("glm-5.2"),
            vec![
                Effort::Off,
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::Xhigh,
                Effort::Max
            ]
        );
        assert!(!provider.verifies_credentials());
    }
}

use goat_auth::CredentialStore;
use goat_provider::{ModelListSource, ProviderMetadata};
use goat_provider_openai_compat::{
    OpenAiCompatProvider, api_key, known_openai_compatible_vision_model,
};

pub const PROVIDER_ID: &str = "vercel";

const BASE_URL: &str = "https://ai-gateway.vercel.sh/v1";
const HOST: &str = "ai-gateway.vercel.sh";
const ENV_VAR: &str = "AI_GATEWAY_API_KEY";

const SETUP: &[&str] = &[
    "Vercel AI Gateway: one key fronting every upstream provider.",
    "Use `AI_GATEWAY_API_KEY` or `goat provider login vercel --key vck_...`.",
    "Models are addressed as `creator/model`, for example `anthropic/claude-opus-5`.",
];

const CATALOG: &[&str] = &[
    "anthropic/claude-opus-5",
    "anthropic/claude-sonnet-5",
    "openai/gpt-5.6",
    "google/gemini-3.6-flash",
    "zai/glm-5.2",
    "moonshotai/kimi-k3",
    "xai/grok-4.5",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("anthropic/claude-opus-5", 1_000_000),
    ("anthropic/claude-sonnet-5", 1_000_000),
    ("openai/gpt-5.6", 1_050_000),
    ("google/gemini-3.6-flash", 1_048_576),
    ("zai/glm-5.2", 1_000_000),
    ("moonshotai/kimi-k3", 1_000_000),
    ("xai/grok-4.5", 500_000),
];

fn is_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("embedding")
        || id.contains("moderation")
        || id.contains("image")
        || id.contains("tts")
        || id.contains("whisper")
        || id.contains("transcribe"))
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
        .with_metadata(ProviderMetadata {
            env_var: Some(ENV_VAR),
            validation: "network",
            endpoint: Some(BASE_URL),
            oauth: Some("not supported"),
            login_endpoint: None,
            setup: SETUP,
        })
}

#[cfg(test)]
mod tests {
    use goat_auth::CredentialStore;
    use goat_provider::{ModelListSource, Provider};

    use super::*;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn gateway_discovers_models_live() {
        let store = store("goat-provider-vercel.json");
        let provider = build(&store, "default");
        assert_eq!(provider.metadata().env_var, Some(ENV_VAR));
        assert_eq!(provider.model_list_source(), ModelListSource::Discover);
        assert_eq!(
            provider.context_window("anthropic/claude-opus-5"),
            Some(1_000_000)
        );
        assert!(provider.supports_images("anthropic/claude-opus-5"));
    }

    #[test]
    fn drops_non_chat_models() {
        for id in [
            "openai/text-embedding-3-large",
            "black-forest-labs/flux-image",
            "openai/whisper-1",
        ] {
            assert!(!is_chat_model(id), "expected to drop {id}");
        }
        assert!(is_chat_model("anthropic/claude-opus-5"));
    }
}

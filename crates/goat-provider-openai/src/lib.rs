use goat_auth::{CredentialKey, CredentialStore};
use goat_provider::{AuthMethod, ProviderId, ProviderMetadata};
use goat_provider_openai_compat::ResponsesProvider;

pub const PROVIDER_ID: &str = "openai";
const BASE_URL: &str = "https://api.openai.com/v1";
const ENV_VAR: &str = "OPENAI_API_KEY";
const SEARCH_MODEL: &str = "gpt-4.1";

const CATALOG: &[&str] = &[
    "gpt-5.6",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-4.1",
    "o3",
    "o4-mini",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("gpt-5", 400_000),
    ("gpt-4.1", 1_047_576),
    ("o3", 200_000),
    ("o4", 200_000),
];

const NON_CHAT_MARKERS: [&str; 15] = [
    "image",
    "audio",
    "tts",
    "whisper",
    "transcribe",
    "realtime",
    "embedding",
    "moderation",
    "search",
    "dall-e",
    "instruct",
    "babbage",
    "davinci",
    "sora",
    "computer-use",
];

fn is_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    if NON_CHAT_MARKERS.iter().any(|marker| id.contains(marker)) {
        return false;
    }
    let mut chars = id.chars();
    id.starts_with("gpt-")
        || (chars.next() == Some('o') && chars.next().is_some_and(|c| c.is_ascii_digit()))
}

pub fn build(store: &CredentialStore, account: &str) -> ResponsesProvider {
    let key = CredentialKey::model(PROVIDER_ID, account);
    let bearer = store
        .resolve(&key, Some(ENV_VAR))
        .map(|cred| cred.bearer().to_owned());
    ResponsesProvider::new(
        ProviderId::from(PROVIDER_ID),
        BASE_URL,
        bearer,
        AuthMethod::ApiKey,
    )
    .with_model_filter(is_chat_model)
    .with_vision_filter(goat_provider_openai_compat::known_openai_vision_model)
    .with_catalog(CATALOG)
    .with_context_windows(CONTEXT_WINDOWS)
    .with_search_model(SEARCH_MODEL)
    .with_metadata(ProviderMetadata {
        env_var: Some(ENV_VAR),
        validation: "network",
        endpoint: None,
        oauth: Some("not supported"),
        login_endpoint: None,
        setup: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::is_chat_model;

    #[test]
    fn keeps_chat_models() {
        for id in [
            "gpt-5.5",
            "gpt-5-codex",
            "gpt-4o",
            "gpt-4.1-mini",
            "o3",
            "o4-mini",
            "gpt-3.5-turbo",
        ] {
            assert!(is_chat_model(id), "expected to keep {id}");
        }
    }

    #[test]
    fn drops_non_chat_models() {
        for id in [
            "gpt-4o-mini-tts",
            "gpt-4o-transcribe",
            "whisper-1",
            "tts-1-hd",
            "gpt-image-1",
            "text-embedding-3-large",
            "gpt-realtime",
            "gpt-4o-search-preview",
            "gpt-3.5-turbo-instruct",
            "omni-moderation-latest",
            "davinci-002",
            "sora-2",
        ] {
            assert!(!is_chat_model(id), "expected to drop {id}");
        }
    }
}

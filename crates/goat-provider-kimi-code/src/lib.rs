mod oauth;

use async_trait::async_trait;
use goat_auth::{Credential, CredentialKey, CredentialStore, TokenSet};
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, Effort, Model, Provider, ProviderId, ProviderMetadata,
    Request, StreamError, ValidateError, Validated, WebSearchOutput,
};
use goat_provider_openai_compat::{
    ChatDiscovery, OpenAiCompatProvider, enforce_https_host, no_vision,
};
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "kimi-code";

const BASE_URL: &str = "https://api.kimi.com/coding/v1";
const ALLOWED_HOST: &str = "api.kimi.com";

const SETUP: &[&str] = &[
    "Kimi Code OAuth device-code login.",
    "Run `goat provider login kimi-code`, open the URL, and enter the code.",
];

const CATALOG: &[&str] = &[
    "k3",
    "k3-256k",
    "kimi-for-coding",
    "kimi-for-coding-highspeed",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("k3-256k", 262_144),
    ("k3", 1_048_576),
    ("kimi-for-coding", 262_144),
];

pub fn build(store: &CredentialStore, account: &str) -> KimiCodeProvider {
    enforce_https_host(BASE_URL, ALLOWED_HOST).expect("kimi-code provider base URL");
    KimiCodeProvider::new(store.clone(), CredentialKey::model(PROVIDER_ID, account))
}

pub struct KimiCodeProvider {
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
}

impl KimiCodeProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            store,
            key,
            client: goat_provider_openai_compat::http_client(),
        }
    }

    fn chat_provider(&self, token: String) -> OpenAiCompatProvider {
        OpenAiCompatProvider::new(
            ProviderId::from(PROVIDER_ID),
            BASE_URL,
            Some(token),
            AuthMethod::OAuth,
        )
        .with_client(self.client.clone())
        .with_extra_headers(oauth::identity_headers())
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT_WINDOWS)
        .with_vision_filter(no_vision)
        .with_images(false)
        .with_efforts(kimi_code_efforts)
        .with_discovery(ChatDiscovery::CatalogOnly)
    }
}

#[async_trait]
impl Provider for KimiCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::OAuth,
            images: false,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            env_var: None,
            validation: "network",
            endpoint: Some(BASE_URL),
            oauth: Some("device code"),
            login_endpoint: None,
            setup: SETUP,
        }
    }

    fn authenticated(&self) -> bool {
        self.store
            .get(&self.key)
            .is_some_and(|cred| matches!(cred, Credential::OAuth(_)))
    }

    fn list_models(&self) -> Vec<String> {
        CATALOG.iter().map(|id| (*id).to_owned()).collect()
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        kimi_code_efforts(model)
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        CONTEXT_WINDOWS
            .iter()
            .find_map(|(prefix, window)| model.starts_with(prefix).then_some(*window))
    }

    fn supports_images(&self, _model: &str) -> bool {
        false
    }

    fn verifies_credentials(&self) -> bool {
        true
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        let store = self.store.clone();
        let key = self.key.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let Some(token) = oauth::current_token(&store, &key).await else {
                return Err(ValidateError::NoCredentials);
            };
            let response = client
                .get(format!("{BASE_URL}/models"))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| ValidateError::unreachable("could not reach provider"))?;
            let status = response.status();
            if status.is_success() {
                Ok(Validated::Live)
            } else if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                Err(ValidateError::InvalidCredentials)
            } else {
                Err(ValidateError::unreachable(format!(
                    "could not reach provider: {status}"
                )))
            }
        })
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let Some(token) = oauth::current_token(&self.store, &self.key).await else {
            return Err(StreamError::auth("no credentials"));
        };
        self.chat_provider(token).stream(req).await
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        tokio::spawn(async move {
            for id in CATALOG {
                if out
                    .send(Model {
                        id: (*id).to_owned(),
                        supports_images: false,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { oauth::login(&status).await.map_err(|err| err.to_string()) })
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let _ = query;
        tokio::spawn(async { Err(StreamError::other("web search is not supported")) })
    }
}

fn kimi_code_efforts(model: &str) -> Vec<Effort> {
    if model.starts_with("k3") {
        vec![Effort::Low, Effort::High, Effort::Max]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use goat_auth::CredentialStore;
    use goat_provider::{AuthMethod, Provider};

    use super::*;
    use crate::oauth::valid_kimi_verification_url;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn kimi_code_is_oauth_provider() {
        let store = store("goat-provider-kimi-code.json");
        let provider = build(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::OAuth);
        assert_eq!(provider.metadata().oauth, Some("device code"));
        assert!(!provider.authenticated());
        assert_eq!(provider.list_models(), CATALOG);
        assert_eq!(provider.context_window("k3"), Some(1_048_576));
        assert_eq!(provider.context_window("k3-256k"), Some(262_144));
        assert_eq!(provider.context_window("kimi-for-coding"), Some(262_144));
        assert_eq!(
            provider.context_window("kimi-for-coding-highspeed"),
            Some(262_144)
        );
        assert_eq!(
            provider.efforts("k3"),
            vec![Effort::Low, Effort::High, Effort::Max]
        );
        assert!(provider.efforts("kimi-for-coding").is_empty());
        assert!(valid_kimi_verification_url(
            "https://auth.kimi.com/device?code=abc"
        ));
        assert!(!valid_kimi_verification_url(
            "https://example.com/device?code=abc"
        ));
    }
}

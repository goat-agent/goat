mod oauth;

use async_trait::async_trait;
use goat_auth::{Credential, CredentialKey, CredentialStore, TokenSet};
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, Effort, Model, Provider, ProviderId, ProviderMetadata,
    Request, StreamError, ValidateError, Validated, WebSearchOutput,
};
use goat_provider_openai_compat::{
    OpenAiCompatProvider, ResponsesProvider, enforce_https_host, no_efforts,
};
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "xai";

const BASE_URL: &str = "https://api.x.ai/v1";
const ALLOWED_HOST: &str = "api.x.ai";

const SETUP: &[&str] = &[
    "xAI Grok provider (API key or SuperGrok / X Premium+ OAuth).",
    "API key: `goat provider login xai --key xai-...` or `XAI_API_KEY`.",
    "OAuth: `goat provider login xai` (browser or device code; no API key).",
    "OAuth coding models (Composer, Grok Build) use the same api.x.ai Responses API.",
];

const OAUTH_CATALOG: &[&str] = &[
    "grok-composer-2.5-fast",
    "grok-4.5",
    "grok-4.3",
    "grok-build-0.1",
    "grok-4.20-0309-reasoning",
    "grok-4.20-0309-non-reasoning",
    "grok-4.20-multi-agent-0309",
];

const API_KEY_CATALOG: &[&str] = &[
    "grok-4.5",
    "grok-4.3",
    "grok-build-0.1",
    "grok-4.20-0309-reasoning",
    "grok-4.20-0309-non-reasoning",
    "grok-4.20-multi-agent-0309",
];

const CATALOG: &[&str] = &[
    "grok-composer-2.5-fast",
    "grok-4.5",
    "grok-4.3",
    "grok-build-0.1",
    "grok-4.20-0309-reasoning",
    "grok-4.20-0309-non-reasoning",
    "grok-4.20-multi-agent-0309",
];

const GROK_CLI_HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "xai-grok-cli"),
    ("x-grok-client-version", "0.2.82"),
    ("x-grok-client-identifier", "xai-grok-cli"),
];

const OAUTH_CONTEXT: &[(&str, u32)] = &[
    ("grok-composer", 200_000),
    ("grok-4.5", 500_000),
    ("grok-4.3", 1_000_000),
    ("grok-build", 256_000),
    ("grok-4.20", 1_000_000),
];

const API_KEY_CONTEXT: &[(&str, u32)] = &[
    ("grok-4.5", 500_000),
    ("grok-4.3", 1_000_000),
    ("grok-4.20", 1_000_000),
    ("grok-build", 256_000),
];

pub fn build(store: &CredentialStore, account: &str) -> XaiProvider {
    enforce_https_host(BASE_URL, ALLOWED_HOST).expect("xai provider base URL");
    XaiProvider::new(store.clone(), CredentialKey::model(PROVIDER_ID, account))
}

enum XaiAuth {
    ApiKey(String),
    OAuth(String),
}

pub struct XaiProvider {
    store: CredentialStore,
    key: CredentialKey,
}

impl XaiProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self { store, key }
    }

    async fn resolve_auth(&self) -> Option<XaiAuth> {
        let cred = self.store.resolve(&self.key, Some("XAI_API_KEY"))?;
        Self::auth_from_credential(&self.store, &self.key, cred).await
    }

    async fn resolve_auth_for_model(&self, model: &str) -> Option<XaiAuth> {
        if Self::is_oauth_model(model)
            && let Some(cred @ Credential::OAuth(_)) = self.store.get(&self.key)
        {
            return Self::auth_from_credential(&self.store, &self.key, cred).await;
        }
        self.resolve_auth().await
    }

    async fn auth_from_credential(
        store: &CredentialStore,
        key: &CredentialKey,
        cred: Credential,
    ) -> Option<XaiAuth> {
        match cred {
            Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. } => {
                Some(XaiAuth::ApiKey(secret.expose().to_owned()))
            }
            Credential::OAuth(_) => oauth::current_oauth_token(store, key)
                .await
                .map(XaiAuth::OAuth),
        }
    }

    fn is_oauth_model(model: &str) -> bool {
        OAUTH_CATALOG.contains(&model)
    }

    fn chat_provider(bearer: String) -> OpenAiCompatProvider {
        OpenAiCompatProvider::new(
            ProviderId::from(PROVIDER_ID),
            BASE_URL,
            Some(bearer),
            AuthMethod::ApiKeyOrOAuth,
        )
        .with_catalog(API_KEY_CATALOG)
        .with_context_windows(API_KEY_CONTEXT)
        .with_vision_filter(vision_model)
        .with_efforts(no_efforts)
        .with_reasoning_effort(false)
        .with_extra_headers(GROK_CLI_HEADERS.iter().copied())
    }

    fn responses_provider(bearer: String) -> ResponsesProvider {
        ResponsesProvider::new(
            ProviderId::from(PROVIDER_ID),
            BASE_URL,
            Some(bearer),
            AuthMethod::ApiKeyOrOAuth,
        )
        .with_catalog(OAUTH_CATALOG)
        .with_context_windows(OAUTH_CONTEXT)
        .with_vision_filter(vision_model)
        .with_model_filter(oauth_chat_model)
        .with_extra_headers(GROK_CLI_HEADERS.iter().copied())
    }

    async fn emit_models(provider: &XaiProvider, out: &mpsc::Sender<Model>) -> bool {
        for id in provider.list_models() {
            if out
                .send(Model {
                    id: id.clone(),
                    supports_images: vision_model(&id),
                })
                .await
                .is_err()
            {
                return false;
            }
        }
        true
    }
}

#[async_trait]
impl Provider for XaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::ApiKeyOrOAuth,
            images: true,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            env_var: Some("XAI_API_KEY"),
            validation: "network",
            endpoint: None,
            oauth: Some("browser or device code (SuperGrok / X Premium+)"),
            login_endpoint: None,
            setup: SETUP,
        }
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, Some("XAI_API_KEY")).is_some()
    }

    fn list_models(&self) -> Vec<String> {
        match self.store.get(&self.key) {
            Some(Credential::ApiKey(_) | Credential::ApiKeyWithEndpoint { .. }) => {
                API_KEY_CATALOG.iter().map(|id| (*id).to_owned()).collect()
            }
            Some(Credential::OAuth(_)) => OAUTH_CATALOG.iter().map(|id| (*id).to_owned()).collect(),
            None => CATALOG.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        if Self::is_oauth_model(model) {
            oauth_efforts(model)
        } else {
            no_efforts(model)
        }
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        if Self::is_oauth_model(model) {
            OAUTH_CONTEXT
                .iter()
                .find_map(|(prefix, window)| model.starts_with(prefix).then_some(*window))
        } else {
            API_KEY_CONTEXT
                .iter()
                .find_map(|(prefix, window)| model.starts_with(prefix).then_some(*window))
        }
    }

    fn supports_images(&self, model: &str) -> bool {
        vision_model(model)
    }

    fn verifies_credentials(&self) -> bool {
        true
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let provider = XaiProvider { store, key };
            let Some(auth) = provider.resolve_auth().await else {
                return Err(ValidateError::NoCredentials);
            };
            match auth {
                XaiAuth::ApiKey(bearer) => XaiProvider::chat_provider(bearer)
                    .validate()
                    .await
                    .expect("validate panicked"),
                XaiAuth::OAuth(bearer) => XaiProvider::responses_provider(bearer)
                    .validate()
                    .await
                    .expect("validate panicked"),
            }
        })
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let store = self.store.clone();
        let key = self.key.clone();
        let model = req.model.clone();
        let provider = XaiProvider { store, key };
        let Some(auth) = provider.resolve_auth_for_model(&model).await else {
            return Err(StreamError::auth("no credentials"));
        };
        match auth {
            XaiAuth::ApiKey(bearer) => XaiProvider::chat_provider(bearer).stream(req).await,
            XaiAuth::OAuth(bearer) => {
                if !XaiProvider::is_oauth_model(&model) && API_KEY_CATALOG.contains(&model.as_str())
                {
                    return Err(StreamError::invalid_request(format!(
                        "model {model} requires an xAI API key; OAuth supports {}",
                        OAUTH_CATALOG.join(", ")
                    )));
                }
                XaiProvider::responses_provider(bearer).stream(req).await
            }
        }
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let provider = XaiProvider { store, key };
            let _ = XaiProvider::emit_models(&provider, &out).await;
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

fn oauth_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("embedding")
        || id.contains("tts")
        || id.contains("whisper")
        || id.contains("image")
        || id.contains("video"))
}

fn oauth_efforts(model: &str) -> Vec<Effort> {
    let id = model.to_ascii_lowercase();
    if id.starts_with("grok-4.3") {
        vec![Effort::Low, Effort::Medium, Effort::High]
    } else {
        Vec::new()
    }
}

fn vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    if id.starts_with("grok-composer") {
        return false;
    }
    id.starts_with("grok-4") || id.contains("vision")
}

#[cfg(test)]
mod tests {
    use goat_auth::CredentialStore;
    use goat_provider::{AuthMethod, Effort, Provider};

    use super::*;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn xai_supports_api_key_and_oauth() {
        let store = store("goat-provider-xai.json");
        let provider = build(&store, "default");
        assert_eq!(provider.capabilities().auth, AuthMethod::ApiKeyOrOAuth);
        assert_eq!(
            provider.metadata().oauth,
            Some("browser or device code (SuperGrok / X Premium+)")
        );
        assert!(!provider.authenticated());
        assert_eq!(provider.list_models(), CATALOG);
        assert_eq!(
            provider.context_window("grok-composer-2.5-fast"),
            Some(200_000)
        );
        assert_eq!(provider.context_window("grok-4.3"), Some(1_000_000));
        assert_eq!(provider.context_window("grok-4.5"), Some(500_000));
        assert_eq!(provider.context_window("grok-build-0.1"), Some(256_000));
        assert!(!provider.supports_images("grok-composer-2.5-fast"));
        assert_eq!(
            provider.efforts("grok-4.3"),
            vec![Effort::Low, Effort::Medium, Effort::High]
        );
    }

    #[test]
    fn list_models_follows_credential_kind() {
        use goat_auth::{Credential, CredentialKey, SecretString, TokenSet};

        let store = store("goat-provider-xai-list.json");
        let oauth = build(&store, "oauth");
        assert_eq!(oauth.list_models().len(), CATALOG.len());

        store
            .store(
                &CredentialKey::model(PROVIDER_ID, "oauth"),
                Credential::OAuth(TokenSet::from_parts(
                    "access".to_owned(),
                    Some("refresh".to_owned()),
                    Some(3600),
                    None,
                )),
            )
            .unwrap();
        let oauth = build(&store, "oauth");
        assert_eq!(
            oauth.list_models(),
            OAUTH_CATALOG
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );

        store
            .store(
                &CredentialKey::model(PROVIDER_ID, "api"),
                Credential::ApiKey(SecretString::from("xai-key".to_owned())),
            )
            .unwrap();
        let api = build(&store, "api");
        assert_eq!(
            api.list_models(),
            API_KEY_CATALOG
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn oauth_model_prefers_stored_oauth_for_composer() {
        use goat_auth::{Credential, CredentialKey, TokenSet};

        let store = store("goat-provider-xai-oauth-pref.json");
        store
            .store(
                &CredentialKey::model(PROVIDER_ID, "default"),
                Credential::OAuth(TokenSet::from_parts(
                    "oauth-access".to_owned(),
                    None,
                    Some(3600),
                    None,
                )),
            )
            .unwrap();
        let provider = XaiProvider::new(store, CredentialKey::model(PROVIDER_ID, "default"));
        let auth = provider
            .resolve_auth_for_model("grok-composer-2.5-fast")
            .await
            .expect("oauth should win for composer");
        assert!(matches!(auth, XaiAuth::OAuth(token) if token == "oauth-access"));
    }
}

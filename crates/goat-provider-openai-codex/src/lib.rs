use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use goat_auth::{
    BASE64URL, Credential, CredentialKey, CredentialStore, Pkce, TokenSet, capture_loopback,
    ensure_valid, random_state,
};
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, Model, Provider, ProviderId, ProviderMetadata, Request,
    StreamError, ValidateError, Validated, WebSearchOutput, now_secs,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

pub const PROVIDER_ID: &str = "openai-codex";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CALLBACK_PORT: u16 = 1455;
const SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const ORIGINATOR: &str = "codex_cli_rs";
const CLIENT_VERSION: &str = "0.133.0";
const BASE: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_INSTRUCTIONS: &str = "You are goat, a coding assistant running in a terminal.";
const DEVICE_USERCODE: &str = "https://auth.openai.com/deviceauth/usercode";

const CATALOG: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.3-codex-spark",
    "gpt-5.4",
    "gpt-5.4-mini",
];
const SEARCH_MODEL: &str = "gpt-5.6-luna";

const CONTEXT_WINDOWS: &[(&str, u32)] = &[("gpt-5.3-codex-spark", 128_000), ("gpt-5", 272_000)];
const DEVICE_TOKEN: &str = "https://auth.openai.com/deviceauth/token";
const DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";

#[derive(Debug, thiserror::Error)]
pub enum CodexError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("auth error: {0}")]
    Auth(#[from] goat_auth::AuthError),
    #[error("url error: {0}")]
    Url(String),
    #[error("token error: {0}")]
    Token(String),
    #[error("no browser available")]
    NoBrowser,
}

fn authorize_url(challenge: &str, state: &str) -> Result<String, CodexError> {
    reqwest::Url::parse_with_params(
        AUTHORIZE,
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", ORIGINATOR),
            ("state", state),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|err| CodexError::Url(err.to_string()))
}

fn codex_user_agent() -> String {
    format!(
        "{ORIGINATOR}/{CLIENT_VERSION} ({} {}; {}) goat/{}",
        std::env::consts::OS,
        std::env::consts::FAMILY,
        std::env::consts::ARCH,
        env!("CARGO_PKG_VERSION")
    )
}

fn account_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let bytes = BASE64URL.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_owned)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

fn auth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

async fn exchange_code(code: &str, verifier: &str) -> Result<TokenSet, CodexError> {
    let response = auth_client()
        .post(TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .await?;
    if let Err(err) = response.error_for_status_ref() {
        return Err(CodexError::Token(err.to_string()));
    }
    let tokens: TokenResponse = response.json().await?;
    Ok(TokenSet::from_parts(
        tokens.access_token,
        tokens.refresh_token,
        tokens.expires_in,
        None,
    ))
}

fn browser_available() -> bool {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub async fn login(status: &mpsc::Sender<String>) -> Result<TokenSet, CodexError> {
    if browser_available() {
        match login_browser().await {
            Err(CodexError::NoBrowser) => login_device(status).await,
            other => other,
        }
    } else {
        login_device(status).await
    }
}

async fn login_browser() -> Result<TokenSet, CodexError> {
    let pkce = Pkce::generate();
    let state = random_state();
    let url = authorize_url(&pkce.challenge, &state)?;
    if open::that(&url).is_err() {
        return Err(CodexError::NoBrowser);
    }
    let code = capture_loopback(CALLBACK_PORT, &state).await?.code;
    exchange_code(&code, &pkce.verifier).await
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    user_code: String,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct DevicePollResponse {
    authorization_code: Option<String>,
    code_verifier: Option<String>,
}

#[derive(Deserialize)]
struct DevicePollError {
    error: Option<String>,
}

async fn login_device(status: &mpsc::Sender<String>) -> Result<TokenSet, CodexError> {
    let client = auth_client();
    let response = client
        .post(DEVICE_USERCODE)
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await?;
    if let Err(err) = response.error_for_status_ref() {
        return Err(CodexError::Token(err.to_string()));
    }
    let device: DeviceCodeResponse = response.json().await?;
    let _ = open::that(DEVICE_VERIFY_URL);
    let _ = status
        .send(format!(
            "open {DEVICE_VERIFY_URL} and enter code: {}",
            device.user_code
        ))
        .await;

    let interval = device.interval.unwrap_or(5).max(1);
    let deadline = now_secs() + 900;
    loop {
        if now_secs() > deadline {
            return Err(CodexError::Token("device login timed out".to_owned()));
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let poll = client
            .post(DEVICE_TOKEN)
            .json(&serde_json::json!({
                "device_auth_id": device.device_auth_id,
                "user_code": device.user_code,
            }))
            .send()
            .await?;
        if !poll.status().is_success() {
            let bytes = poll.bytes().await.unwrap_or_default();
            if let Ok(err_body) = serde_json::from_slice::<DevicePollError>(&bytes) {
                match err_body.error.as_deref() {
                    Some("access_denied") => {
                        return Err(CodexError::Token("device login access denied".to_owned()));
                    }
                    Some("expired_token") => {
                        return Err(CodexError::Token("device login code expired".to_owned()));
                    }
                    _ => {}
                }
            }
            continue;
        }
        let Ok(body) = poll.json::<DevicePollResponse>().await else {
            continue;
        };
        if let (Some(code), Some(verifier)) = (body.authorization_code, body.code_verifier) {
            return exchange_code(&code, &verifier).await;
        }
    }
}

async fn do_refresh(refresh_token: String) -> Result<TokenSet, String> {
    let response = auth_client()
        .post(TOKEN)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if let Err(err) = response.error_for_status_ref() {
        return Err(err.to_string());
    }
    let tokens: TokenResponse = response.json().await.map_err(|e| e.to_string())?;
    Ok(TokenSet::from_parts(
        tokens.access_token,
        tokens.refresh_token,
        tokens.expires_in,
        Some(refresh_token.as_str()),
    ))
}

async fn current_access(
    store: &CredentialStore,
    key: &CredentialKey,
) -> Option<(String, Option<String>)> {
    let tokens = match store.resolve(key, None)? {
        Credential::OAuth(tokens) => tokens,
        Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. } => {
            let access = secret.expose().to_owned();
            let account = account_id(&access);
            return Some((access, account));
        }
    };
    let tokens = ensure_valid(tokens, store, key, do_refresh).await?;
    let access = tokens.access_token.expose().to_owned();
    let account = account_id(&access);
    Some((access, account))
}

pub fn build(store: &CredentialStore, account: &str) -> CodexProvider {
    let key = CredentialKey::model(PROVIDER_ID, account);
    CodexProvider::new(store.clone(), key)
}

pub struct CodexProvider {
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
}

impl CodexProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self {
            store,
            key,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_mins(5))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
        }
    }
}

async fn fetch_models(client: &reqwest::Client, access: &str, account: Option<&str>) -> Vec<Model> {
    let mut builder = client
        .get(format!("{BASE}/models?client_version={CLIENT_VERSION}"))
        .bearer_auth(access)
        .header("Accept", "application/json");
    if let Some(account) = account {
        builder = builder.header("chatgpt-account-id", account);
    }
    let Ok(response) = builder.send().await else {
        return Vec::new();
    };
    if !response.status().is_success() {
        return Vec::new();
    }
    let Ok(value) = response.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    let Some(items) = value.get("models").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|model| model.get("visibility").and_then(serde_json::Value::as_str) == Some("list"))
        .filter_map(|model| {
            let id = model.get("slug").and_then(serde_json::Value::as_str)?;
            Some(Model {
                id: id.to_owned(),
                supports_images: goat_provider_openai_compat::known_openai_vision_model(id),
            })
        })
        .collect()
}

#[derive(Serialize)]
struct CodexSearchRequest<'a> {
    id: &'a str,
    model: &'a str,
    input: Vec<serde_json::Value>,
    commands: CodexSearchCommands<'a>,
    settings: CodexSearchSettings,
    max_output_tokens: u64,
}

#[derive(Serialize)]
struct CodexSearchCommands<'a> {
    search_query: Vec<CodexSearchQuery<'a>>,
    response_length: CodexSearchResponseLength,
}

#[derive(Serialize)]
struct CodexSearchQuery<'a> {
    q: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum CodexSearchResponseLength {
    Short,
}

#[derive(Serialize)]
struct CodexSearchSettings {
    allowed_callers: Vec<CodexSearchAllowedCaller>,
    external_web_access: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CodexSearchAllowedCaller {
    Direct,
}

#[derive(Deserialize)]
struct CodexSearchResponse {
    output: String,
}

fn codex_search_body(id: &str, model: &str, query: &str) -> serde_json::Value {
    serde_json::to_value(CodexSearchRequest {
        id,
        model,
        input: vec![json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": query }],
        })],
        commands: CodexSearchCommands {
            search_query: vec![CodexSearchQuery { q: query }],
            response_length: CodexSearchResponseLength::Short,
        },
        settings: CodexSearchSettings {
            allowed_callers: vec![CodexSearchAllowedCaller::Direct],
            external_web_access: true,
        },
        max_output_tokens: 2500,
    })
    .expect("CodexSearchRequest is always serializable")
}

async fn run_codex_search(
    client: &reqwest::Client,
    access: &str,
    account: Option<&str>,
    query: &str,
) -> Result<WebSearchOutput, StreamError> {
    let url = format!("{BASE}/alpha/search");
    let body = codex_search_body("goat-web-search", SEARCH_MODEL, query);
    let mut builder = client.post(&url).bearer_auth(access).json(&body);
    if let Some(account) = account {
        builder = builder.header("chatgpt-account-id", account);
    }
    let resp = builder
        .send()
        .await
        .map_err(|err| goat_provider_openai_compat::common::transport(&err))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let detail = resp.text().await.unwrap_or_default();
        return Err(goat_provider_openai_compat::common::classify_http(
            status, &headers, &detail,
        ));
    }
    let response: CodexSearchResponse = resp
        .json()
        .await
        .map_err(|err| StreamError::other(format!("invalid search response: {err}")))?;
    let content = if response.output.trim().is_empty() {
        "No results found.".to_owned()
    } else {
        response.output
    };
    Ok(WebSearchOutput {
        content,
        results: Vec::new(),
    })
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, None).is_some()
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            if current_access(&store, &key).await.is_some() {
                Ok(Validated::Assumed)
            } else {
                Err(ValidateError::NoCredentials)
            }
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::OAuth,
            images: true,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            env_var: None,
            validation: "oauth",
            endpoint: None,
            oauth: Some("browser or device"),
            login_endpoint: None,
            setup: &[],
        }
    }

    fn supports_images(&self, model: &str) -> bool {
        goat_provider_openai_compat::known_openai_vision_model(model)
    }

    fn supports_web_search(&self) -> bool {
        true
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some((access, account)) = current_access(&store, &key).await else {
                return Err(StreamError::auth("not logged in to codex"));
            };
            run_codex_search(&client, &access, account.as_deref(), &query).await
        })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { login(&status).await.map_err(|e| e.to_string()) })
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        CONTEXT_WINDOWS
            .iter()
            .find(|(prefix, _)| model.starts_with(prefix))
            .map(|(_, w)| *w)
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let client = self.client.clone();
        let url = format!("{BASE}/responses");
        let store = self.store.clone();
        let key = self.key.clone();
        let Some((access, account)) = current_access(&store, &key).await else {
            return Err(goat_provider::StreamError::auth("not logged in to codex"));
        };
        let body = goat_provider_openai_compat::build_body(
            &req.model,
            &req.messages,
            &req.tools,
            Some(DEFAULT_INSTRUCTIONS),
            false,
            req.effort,
            req.tool_choice,
            req.system.as_deref(),
            req.temperature,
            req.max_tokens,
        );
        let session_id = goat_auth::random_state();
        let user_agent = codex_user_agent();
        goat_provider_openai_compat::run_request(
            &client,
            &url,
            Some(&access),
            account.as_deref(),
            &body,
            Some(goat_provider_openai_compat::parse_codex_ratelimits),
            &[
                ("originator", ORIGINATOR),
                ("session_id", session_id.as_str()),
                ("OpenAI-Beta", "responses=experimental"),
                ("User-Agent", user_agent.as_str()),
            ],
        )
        .await
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn efforts(&self, model: &str) -> Vec<goat_provider::Effort> {
        goat_provider_openai_compat::responses_efforts(model)
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let client = self.client.clone();
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some((access, account)) = current_access(&store, &key).await else {
                return;
            };
            for model in fetch_models(&client, &access, account.as_deref()).await {
                if out.send(model).await.is_err() {
                    return;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{account_id, authorize_url, codex_search_body};

    #[test]
    fn authorize_url_carries_pkce_and_client() {
        let url = authorize_url("CHAL", "STATE").unwrap();
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=STATE"));
        assert!(url.contains("redirect_uri=http"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("id_token_add_organizations=true"));
    }

    #[test]
    fn codex_search_body_matches_upstream_shape() {
        let body = codex_search_body("search-session", "gpt-test", "find this");
        assert_eq!(body["id"], "search-session");
        assert_eq!(body["model"], "gpt-test");
        assert!(body["input"].is_array());
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][0]["text"], "find this");
        assert_eq!(body["commands"]["search_query"][0]["q"], "find this");
        assert_eq!(body["commands"]["response_length"], "short");
        assert_eq!(body["settings"]["allowed_callers"][0], "direct");
        assert_eq!(body["settings"]["external_web_access"], true);
        assert_eq!(body["max_output_tokens"], 2500);
    }

    #[test]
    fn decodes_account_id_from_jwt() {
        use base64::Engine;
        let payload = r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-99"}}"#;
        let encoded = goat_auth::BASE64URL.encode(payload.as_bytes());
        let jwt = format!("header.{encoded}.sig");
        assert_eq!(account_id(&jwt).as_deref(), Some("acct-99"));
    }
}

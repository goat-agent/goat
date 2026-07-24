use goat_auth::{
    Credential, CredentialKey, CredentialStore, Pkce, TokenSet, ensure_valid, random_state,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

use crate::{ENV_VAR, OAUTH_AUTHORIZE, OAUTH_CLIENT_ID, OAUTH_SCOPE, OAUTH_TOKEN, OAUTH_TOKEN_UA};

#[derive(Debug, thiserror::Error)]
pub enum AnthropicAuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("url error: {0}")]
    Url(String),
    #[error("token error: {0}")]
    Token(String),
    #[error("auth error: {0}")]
    Auth(#[from] goat_auth::AuthError),
}

pub(crate) enum Auth {
    ApiKey(String),
    OAuth(String),
}

pub(crate) fn authorize_url(
    challenge: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<String, AnthropicAuthError> {
    reqwest::Url::parse_with_params(
        OAUTH_AUTHORIZE,
        &[
            ("code", "true"),
            ("client_id", OAUTH_CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("scope", OAUTH_SCOPE),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|err| AnthropicAuthError::Url(err.to_string()))
}

pub(crate) async fn do_login(
    status: &mpsc::Sender<String>,
) -> Result<TokenSet, AnthropicAuthError> {
    let pkce = Pkce::generate();
    let state = random_state();
    let (listener, port) = goat_auth::bind_loopback().await?;
    let redirect = format!("http://localhost:{port}/callback");
    let url = authorize_url(&pkce.challenge, &state, &redirect)?;
    let _ = status
        .send(format!(
            "opening browser to sign in\u{2026} if it does not open, visit:\n{url}"
        ))
        .await;
    let _ = open::that(&url);
    let code = goat_auth::capture_on(listener, &state).await?;
    exchange_code(&code, &pkce.verifier, &state, &redirect).await
}

fn auth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

async fn exchange_code(
    code: &str,
    verifier: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<TokenSet, AnthropicAuthError> {
    let response = auth_client()
        .post(OAUTH_TOKEN)
        .header("Accept", "application/json, text/plain, */*")
        .header("User-Agent", OAUTH_TOKEN_UA)
        .json(&json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": state,
            "client_id": OAUTH_CLIENT_ID,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }))
        .send()
        .await?;
    parse_token_response(response)
        .await
        .map(|t| TokenSet::from_parts(t.access_token, t.refresh_token, t.expires_in, None))
}

async fn do_refresh(refresh_token: String) -> Result<TokenSet, String> {
    let response = auth_client()
        .post(OAUTH_TOKEN)
        .header("Accept", "application/json, text/plain, */*")
        .header("User-Agent", OAUTH_TOKEN_UA)
        .json(&json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": OAUTH_CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    parse_token_response(response)
        .await
        .map(|t| {
            TokenSet::from_parts(
                t.access_token,
                t.refresh_token,
                t.expires_in,
                Some(&refresh_token),
            )
        })
        .map_err(|e| e.to_string())
}

async fn parse_token_response(
    response: reqwest::Response,
) -> Result<TokenResponse, AnthropicAuthError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AnthropicAuthError::Token(format!("{status}: {body}")));
    }
    response.json().await.map_err(AnthropicAuthError::Http)
}

pub(crate) async fn current_auth(store: &CredentialStore, key: &CredentialKey) -> Option<Auth> {
    match store.resolve(key, Some(ENV_VAR))? {
        Credential::ApiKey(secret) | Credential::ApiKeyWithEndpoint { secret, .. } => {
            Some(Auth::ApiKey(secret.expose().to_owned()))
        }
        Credential::OAuth(tokens) => {
            let tokens = ensure_valid(tokens, store, key, do_refresh).await?;
            Some(Auth::OAuth(tokens.access_token.expose().to_owned()))
        }
    }
}

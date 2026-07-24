use std::time::Duration;

use goat_auth::{
    Credential, CredentialKey, CredentialStore, Pkce, TokenSet, capture_loopback_code,
    ensure_valid, now_secs, random_state,
};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use tokio::sync::mpsc;

const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const CALLBACK_PORT: u16 = 56121;
const REDIRECT_URI: &str = "http://127.0.0.1:56121/callback";
const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const LOGIN_TIMEOUT_SECS: i64 = 300;

#[derive(Debug, thiserror::Error)]
pub enum XaiOAuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("auth error: {0}")]
    Auth(#[from] goat_auth::AuthError),
    #[error("oauth error: {0}")]
    OAuth(String),
    #[error("no browser available")]
    NoBrowser,
}

struct OAuthDiscovery {
    authorization_endpoint: String,
    token_endpoint: String,
}

struct DeviceDiscovery {
    device_authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    #[serde(rename = "authorization_endpoint")]
    authorization: Option<String>,
    #[serde(rename = "token_endpoint")]
    token: Option<String>,
    #[serde(rename = "device_authorization_endpoint")]
    device_authorization: Option<String>,
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: Option<String>,
}

pub fn trusted_xai_host(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host == "x.ai" || host.ends_with(".x.ai")
}

fn require_trusted_endpoint(endpoint: &str, label: &str) -> Result<String, XaiOAuthError> {
    if trusted_xai_host(endpoint) {
        Ok(endpoint.to_owned())
    } else {
        Err(XaiOAuthError::OAuth(format!(
            "xAI OAuth discovery returned untrusted {label}"
        )))
    }
}

fn oauth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client")
}

fn oauth_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    let ua = format!("goat-code/{}", env!("CARGO_PKG_VERSION"));
    if let Ok(value) = HeaderValue::from_str(&ua) {
        headers.insert(USER_AGENT, value);
    }
    headers
}

async fn fetch_discovery() -> Result<OAuthDiscovery, XaiOAuthError> {
    let client = oauth_client();
    let response = client
        .get(DISCOVERY_URL)
        .headers(oauth_headers())
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(XaiOAuthError::OAuth(format!(
            "OAuth discovery failed: {status}"
        )));
    }
    let doc: DiscoveryDocument = response.json().await?;
    let authorization_endpoint = doc
        .authorization
        .ok_or_else(|| XaiOAuthError::OAuth("missing authorization_endpoint".to_owned()))?;
    let token_endpoint = doc
        .token
        .ok_or_else(|| XaiOAuthError::OAuth("missing token_endpoint".to_owned()))?;
    Ok(OAuthDiscovery {
        authorization_endpoint: require_trusted_endpoint(
            &authorization_endpoint,
            "authorization endpoint",
        )?,
        token_endpoint: require_trusted_endpoint(&token_endpoint, "token endpoint")?,
    })
}

async fn fetch_device_discovery() -> Result<DeviceDiscovery, XaiOAuthError> {
    let client = oauth_client();
    let response = client
        .get(DISCOVERY_URL)
        .headers(oauth_headers())
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(XaiOAuthError::OAuth(format!(
            "OAuth discovery failed: {status}"
        )));
    }
    let doc: DiscoveryDocument = response.json().await?;
    let device_authorization_endpoint = doc
        .device_authorization
        .ok_or_else(|| XaiOAuthError::OAuth("missing device_authorization_endpoint".to_owned()))?;
    let token_endpoint = doc
        .token
        .ok_or_else(|| XaiOAuthError::OAuth("missing token_endpoint".to_owned()))?;
    Ok(DeviceDiscovery {
        device_authorization_endpoint: require_trusted_endpoint(
            &device_authorization_endpoint,
            "device authorization endpoint",
        )?,
        token_endpoint: require_trusted_endpoint(&token_endpoint, "token endpoint")?,
    })
}

pub fn build_authorize_url(
    authorization_endpoint: &str,
    challenge: &str,
    state: &str,
    nonce: &str,
) -> Result<String, XaiOAuthError> {
    let endpoint = require_trusted_endpoint(authorization_endpoint, "authorization endpoint")?;
    reqwest::Url::parse_with_params(
        &endpoint,
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPE),
            ("state", state),
            ("nonce", nonce),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("plan", "generic"),
            ("referrer", "goat-code"),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|err| XaiOAuthError::OAuth(err.to_string()))
}

fn random_nonce() -> String {
    random_state()
}

fn parse_token_response(
    tokens: TokenResponse,
    require_refresh: bool,
) -> Result<TokenSet, XaiOAuthError> {
    if tokens.access_token.is_empty() {
        return Err(XaiOAuthError::OAuth(
            "token response is missing access_token".to_owned(),
        ));
    }
    if require_refresh && tokens.refresh_token.as_deref().is_none_or(str::is_empty) {
        return Err(XaiOAuthError::OAuth(
            "token response is missing refresh_token".to_owned(),
        ));
    }
    Ok(TokenSet::from_parts(
        tokens.access_token,
        tokens.refresh_token,
        tokens.expires_in,
        None,
    ))
}

async fn exchange_authorization_code(
    token_endpoint: &str,
    code: &str,
    pkce: &Pkce,
) -> Result<TokenSet, XaiOAuthError> {
    let endpoint = require_trusted_endpoint(token_endpoint, "token endpoint")?;
    let client = oauth_client();
    let response = client
        .post(endpoint)
        .headers(oauth_headers())
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", pkce.verifier.as_str()),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(XaiOAuthError::OAuth(format!(
            "token exchange failed: {status}"
        )));
    }
    let tokens: TokenResponse = response.json().await?;
    parse_token_response(tokens, true)
}

pub async fn refresh_token(refresh_token: String) -> Result<TokenSet, String> {
    let discovery = fetch_discovery().await.map_err(|err| err.to_string())?;
    let client = oauth_client();
    let response = client
        .post(&discovery.token_endpoint)
        .headers(oauth_headers())
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("token refresh failed: {status}"));
    }
    let tokens: TokenResponse = response.json().await.map_err(|err| err.to_string())?;
    parse_token_response(tokens, false).map_err(|err| err.to_string())
}

pub async fn current_oauth_token(store: &CredentialStore, key: &CredentialKey) -> Option<String> {
    let Credential::OAuth(tokens) = store.get(key)? else {
        return None;
    };
    let tokens = ensure_valid(tokens, store, key, refresh_token).await?;
    Some(tokens.access_token.expose().to_owned())
}

fn browser_available() -> bool {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

async fn login_browser(status: &mpsc::Sender<String>) -> Result<TokenSet, XaiOAuthError> {
    let discovery = fetch_discovery().await?;
    let pkce = Pkce::generate();
    let state = random_state();
    let nonce = random_nonce();
    let url = build_authorize_url(
        &discovery.authorization_endpoint,
        &pkce.challenge,
        &state,
        &nonce,
    )?;
    let _ = status
        .send(format!(
            "opening browser to sign in\u{2026} if it does not open, visit:\n{url}"
        ))
        .await;
    if open::that(&url).is_err() {
        return Err(XaiOAuthError::NoBrowser);
    }
    let code = capture_loopback_code(CALLBACK_PORT, &state).await?;
    exchange_authorization_code(&discovery.token_endpoint, &code, &pkce).await
}

async fn request_device_authorization(
    device_authorization_endpoint: &str,
) -> Result<DeviceAuthorizationResponse, XaiOAuthError> {
    let endpoint = require_trusted_endpoint(
        device_authorization_endpoint,
        "device authorization endpoint",
    )?;
    let client = oauth_client();
    let response = client
        .post(endpoint)
        .headers(oauth_headers())
        .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(XaiOAuthError::OAuth(format!(
            "device authorization failed: {status}"
        )));
    }
    let device: DeviceAuthorizationResponse = response.json().await?;
    if device.user_code.is_empty() || device.device_code.is_empty() {
        return Err(XaiOAuthError::OAuth(
            "device authorization response is missing required fields".to_owned(),
        ));
    }
    Ok(device)
}

pub fn valid_device_verification_url(url: &str) -> bool {
    trusted_xai_host(url)
}

async fn poll_device_token(
    token_endpoint: &str,
    device_code: &str,
    expires_in: u64,
    interval: u64,
) -> Result<TokenSet, XaiOAuthError> {
    let endpoint = require_trusted_endpoint(token_endpoint, "token endpoint")?;
    let client = oauth_client();
    let mut interval_secs = interval.max(1);
    let deadline = now_secs() + i64::try_from(expires_in).unwrap_or(LOGIN_TIMEOUT_SECS);
    loop {
        if now_secs() > deadline {
            return Err(XaiOAuthError::OAuth("device login timed out".to_owned()));
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        let response = client
            .post(&endpoint)
            .headers(oauth_headers())
            .form(&[
                ("grant_type", DEVICE_GRANT_TYPE),
                ("client_id", CLIENT_ID),
                ("device_code", device_code),
            ])
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            let tokens: TokenResponse = response.json().await?;
            return parse_token_response(tokens, true);
        }
        let body: OAuthErrorResponse = response
            .json()
            .await
            .unwrap_or(OAuthErrorResponse { error: None });
        match body.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval_secs = interval_secs.saturating_add(5),
            Some("access_denied" | "authorization_denied") => {
                return Err(XaiOAuthError::OAuth(
                    "device login access denied".to_owned(),
                ));
            }
            Some("expired_token") => {
                return Err(XaiOAuthError::OAuth("device login code expired".to_owned()));
            }
            Some(code) => {
                return Err(XaiOAuthError::OAuth(format!(
                    "device token polling failed: {code}"
                )));
            }
            None => {
                return Err(XaiOAuthError::OAuth(format!(
                    "device token polling failed: {status}"
                )));
            }
        }
    }
}

async fn login_device(status: &mpsc::Sender<String>) -> Result<TokenSet, XaiOAuthError> {
    let discovery = fetch_device_discovery().await?;
    let device = request_device_authorization(&discovery.device_authorization_endpoint).await?;
    let url = device
        .verification_uri_complete
        .as_deref()
        .or(device.verification_uri.as_deref())
        .unwrap_or_default();
    if !valid_device_verification_url(url) {
        return Err(XaiOAuthError::OAuth(
            "device authorization returned an invalid verification URL".to_owned(),
        ));
    }
    let _ = open::that(url);
    let _ = status
        .send(format!("open {url} and enter code: {}", device.user_code))
        .await;
    poll_device_token(
        &discovery.token_endpoint,
        &device.device_code,
        device.expires_in.unwrap_or(900),
        device.interval.unwrap_or(5),
    )
    .await
}

pub async fn login(status: &mpsc::Sender<String>) -> Result<TokenSet, XaiOAuthError> {
    if browser_available() {
        match login_browser(status).await {
            Err(XaiOAuthError::NoBrowser) => login_device(status).await,
            other => other,
        }
    } else {
        login_device(status).await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CLIENT_ID, REDIRECT_URI, SCOPE, build_authorize_url, parse_token_response,
        trusted_xai_host, valid_device_verification_url,
    };

    #[test]
    fn authorize_url_contains_required_params() {
        let url = build_authorize_url(
            "https://auth.x.ai/oauth2/authorize",
            "challenge",
            "state-value",
            "nonce-value",
        )
        .unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!(parsed.origin().ascii_serialization(), "https://auth.x.ai");
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        let value = |key: &str| pairs.get(key).map(|value| value.as_ref().to_owned());
        assert_eq!(value("client_id"), Some(CLIENT_ID.to_owned()));
        assert_eq!(value("redirect_uri"), Some(REDIRECT_URI.to_owned()));
        assert_eq!(value("scope"), Some(SCOPE.to_owned()));
        assert_eq!(value("code_challenge"), Some("challenge".to_owned()));
        assert_eq!(value("state"), Some("state-value".to_owned()));
        assert_eq!(value("nonce"), Some("nonce-value".to_owned()));
        assert_eq!(value("referrer"), Some("goat-code".to_owned()));
    }

    #[test]
    fn rejects_untrusted_authorize_host() {
        let err = build_authorize_url(
            "https://evil.example/oauth2/authorize",
            "challenge",
            "state",
            "nonce",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("untrusted"));
    }

    #[test]
    fn trusted_xai_hosts() {
        assert!(trusted_xai_host("https://auth.x.ai/oauth2/authorize"));
        assert!(trusted_xai_host("https://accounts.x.ai/sign-in"));
        assert!(!trusted_xai_host("https://evil.example/oauth"));
        assert!(!trusted_xai_host("http://auth.x.ai/oauth"));
    }

    #[test]
    fn validates_device_verification_url() {
        assert!(valid_device_verification_url(
            "https://accounts.x.ai/device?code=abc"
        ));
        assert!(!valid_device_verification_url(
            "https://example.com/device?code=abc"
        ));
    }

    #[test]
    fn token_parse_does_not_leak_secrets() {
        let err = parse_token_response(
            super::TokenResponse {
                access_token: String::new(),
                refresh_token: Some("refresh-secret".to_owned()),
                expires_in: Some(3600),
            },
            true,
        )
        .unwrap_err()
        .to_string();
        assert!(!err.contains("refresh-secret"));
    }
}

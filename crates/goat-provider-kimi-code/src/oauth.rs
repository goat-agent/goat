use std::path::PathBuf;

use goat_auth::{Credential, CredentialKey, CredentialStore, TokenSet, ensure_valid, now_secs};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::Deserialize;
use tokio::sync::mpsc;

const OAUTH_HOST: &str = "https://auth.kimi.com";
const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

#[derive(Debug, thiserror::Error)]
pub enum KimiCodeOAuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("oauth error: {0}")]
    OAuth(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    user_code: String,
    device_code: String,
    verification_uri: Option<String>,
    verification_uri_complete: String,
    expires_in: Option<u64>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    scope: Option<String>,
    token_type: Option<String>,
}

#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: Option<String>,
    #[serde(rename = "error_description")]
    _error_description: Option<String>,
}

pub fn oauth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client")
}

pub async fn login(status: &mpsc::Sender<String>) -> Result<TokenSet, KimiCodeOAuthError> {
    let client = oauth_client();
    let device = request_device_authorization(&client).await?;
    let url = if device.verification_uri_complete.is_empty() {
        device.verification_uri.as_deref().unwrap_or("")
    } else {
        &device.verification_uri_complete
    };
    if !valid_kimi_verification_url(url) {
        return Err(KimiCodeOAuthError::OAuth(
            "device authorization returned an invalid verification URL".to_owned(),
        ));
    }
    let _ = status
        .send(format!("open {url} and enter code: {}", device.user_code))
        .await;
    poll_device_token(&client, &device).await
}

pub async fn current_token(store: &CredentialStore, key: &CredentialKey) -> Option<String> {
    let Credential::OAuth(tokens) = store.get(key)? else {
        return None;
    };
    let tokens = ensure_valid(tokens, store, key, refresh).await?;
    Some(tokens.access_token.expose().to_owned())
}

async fn request_device_authorization(
    client: &reqwest::Client,
) -> Result<DeviceAuthorizationResponse, KimiCodeOAuthError> {
    let response = client
        .post(format!("{OAUTH_HOST}/api/oauth/device_authorization"))
        .headers(kimi_headers())
        .form(&[("client_id", CLIENT_ID)])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(KimiCodeOAuthError::OAuth(format!(
            "device authorization failed: {status}"
        )));
    }
    let device: DeviceAuthorizationResponse = response.json().await?;
    if device.user_code.is_empty()
        || device.device_code.is_empty()
        || device.verification_uri_complete.is_empty()
    {
        return Err(KimiCodeOAuthError::OAuth(
            "device authorization response is missing required fields".to_owned(),
        ));
    }
    Ok(device)
}

async fn poll_device_token(
    client: &reqwest::Client,
    device: &DeviceAuthorizationResponse,
) -> Result<TokenSet, KimiCodeOAuthError> {
    let mut interval = device.interval.unwrap_or(5).max(1);
    let deadline = now_secs() + i64::try_from(device.expires_in.unwrap_or(900)).unwrap_or(900);
    loop {
        if now_secs() > deadline {
            return Err(KimiCodeOAuthError::OAuth(
                "device login timed out".to_owned(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        let response = client
            .post(format!("{OAUTH_HOST}/api/oauth/token"))
            .headers(kimi_headers())
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            let tokens: TokenResponse = response.json().await?;
            return parse_token_response(tokens);
        }
        let error = response.json::<OAuthErrorResponse>().await.ok();
        match error.as_ref().and_then(|err| err.error.as_deref()) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = interval.saturating_add(5).min(30),
            Some("expired_token") => {
                return Err(KimiCodeOAuthError::OAuth(
                    "device login code expired".to_owned(),
                ));
            }
            Some("access_denied") => {
                return Err(KimiCodeOAuthError::OAuth(
                    "device login access denied".to_owned(),
                ));
            }
            Some(code) => {
                return Err(KimiCodeOAuthError::OAuth(format!(
                    "device token polling failed: {code}"
                )));
            }
            None => {
                return Err(KimiCodeOAuthError::OAuth(format!(
                    "device token polling failed: {status}"
                )));
            }
        }
    }
}

async fn refresh(refresh_token: String) -> Result<TokenSet, String> {
    let client = oauth_client();
    let response = client
        .post(format!("{OAUTH_HOST}/api/oauth/token"))
        .headers(kimi_headers())
        .form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await
        .map_err(|_| "token refresh request failed".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("token refresh failed: {status}"));
    }
    response
        .json::<TokenResponse>()
        .await
        .map_err(|_| "token refresh returned invalid JSON".to_owned())
        .and_then(|tokens| parse_token_response(tokens).map_err(|err| err.to_string()))
}

pub(crate) fn parse_token_response(tokens: TokenResponse) -> Result<TokenSet, KimiCodeOAuthError> {
    let _ = (&tokens.scope, &tokens.token_type);
    if tokens.access_token.is_empty() || tokens.refresh_token.is_empty() || tokens.expires_in <= 0 {
        return Err(KimiCodeOAuthError::OAuth(
            "token response is missing required fields".to_owned(),
        ));
    }
    Ok(TokenSet::from_parts(
        tokens.access_token,
        Some(tokens.refresh_token),
        Some(tokens.expires_in),
        None,
    ))
}

pub fn identity_headers() -> Vec<(String, String)> {
    vec![
        (
            USER_AGENT.as_str().to_owned(),
            format!("goat-code/{}", env!("CARGO_PKG_VERSION")),
        ),
        ("x-msh-platform".to_owned(), "kimi_code_cli".to_owned()),
        (
            "x-msh-version".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
        ),
        ("x-msh-device-name".to_owned(), ascii_header(&device_name())),
        (
            "x-msh-device-model".to_owned(),
            ascii_header(&device_model()),
        ),
        (
            "x-msh-os-version".to_owned(),
            ascii_header(std::env::consts::OS),
        ),
        ("x-msh-device-id".to_owned(), ascii_header(&device_id())),
    ]
}

fn kimi_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    for (name, value) in identity_headers() {
        let Ok(name) = HeaderName::try_from(name.as_str()) else {
            continue;
        };
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(name, value);
        }
    }
    headers
}

fn ascii_header(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| matches!(*ch as u32, 0x20..=0x7e))
        .collect::<String>()
        .trim()
        .to_owned();
    if cleaned.is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}

fn device_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

fn device_model() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

fn device_id() -> String {
    let path = device_id_path();
    if let Ok(value) = std::fs::read_to_string(&path) {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_owned();
        }
    }
    let id = goat_auth::random_state();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        set_private_dir(parent);
    }
    if std::fs::write(&path, &id).is_ok() {
        set_private_file(&path);
    }
    id
}

fn device_id_path() -> PathBuf {
    std::env::home_dir().map_or_else(
        || PathBuf::from(".goat-code-kimi-code-device-id"),
        |home| home.join(".goat-code").join("kimi-code-device-id"),
    )
}

#[cfg(unix)]
fn set_private_dir(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_dir(_path: &std::path::Path) {}

#[cfg(unix)]
fn set_private_file(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_file(_path: &std::path::Path) {}

pub fn valid_kimi_verification_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            (url.scheme() == "https").then(|| {
                url.host_str()
                    .is_some_and(|host| host == "kimi.com" || host.ends_with(".kimi.com"))
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{TokenResponse, parse_token_response, valid_kimi_verification_url};

    #[test]
    fn accepts_kimi_verification_hosts() {
        assert!(valid_kimi_verification_url(
            "https://www.kimi.com/code/authorize_device?user_code=J2V4-A52W"
        ));
        assert!(valid_kimi_verification_url("https://auth.kimi.com/device"));
        assert!(valid_kimi_verification_url("https://kimi.com/device"));
        assert!(!valid_kimi_verification_url("http://www.kimi.com/device"));
        assert!(!valid_kimi_verification_url(
            "https://kimi.com.evil.io/device"
        ));
        assert!(!valid_kimi_verification_url("https://evil.io/device"));
    }

    #[test]
    fn parses_kimi_token_response_without_leaking_secrets() {
        let token = parse_token_response(TokenResponse {
            access_token: "access-secret".to_owned(),
            refresh_token: "refresh-secret".to_owned(),
            expires_in: 3600,
            scope: Some("scope".to_owned()),
            token_type: Some("Bearer".to_owned()),
        })
        .unwrap();
        assert_eq!(token.access_token.expose(), "access-secret");
        assert_eq!(token.refresh_token.unwrap().expose(), "refresh-secret");
        let error = parse_token_response(TokenResponse {
            access_token: String::new(),
            refresh_token: "refresh-secret".to_owned(),
            expires_in: 3600,
            scope: None,
            token_type: None,
        })
        .unwrap_err()
        .to_string();
        assert!(!error.contains("refresh-secret"));
        assert!(!error.contains("access-secret"));
    }
}

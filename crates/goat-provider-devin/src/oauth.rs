use goat_auth::{AuthError, Pkce, TokenSet, bind_loopback, capture_on, random_state};
use serde_json::json;
use tokio::sync::mpsc;

use crate::client::{DEVIN_API_URL, WEBAPP_HOST, normalize_session_token};

const AUTHORIZE: &str = "/auth/cli/continue";
const TOKEN_EXCHANGE: &str = "/auth/cli/token";

#[derive(Debug, thiserror::Error)]
pub enum DevinLoginError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("auth error: {0}")]
    Auth(#[from] AuthError),
    #[error("url error: {0}")]
    Url(String),
    #[error("token exchange failed: {0}")]
    Exchange(String),
    #[error("no browser available")]
    NoBrowser,
}

fn authorize_url(port: u16, challenge: &str, state: &str) -> Result<String, DevinLoginError> {
    let redirect = format!("http://127.0.0.1:{port}/callback");
    reqwest::Url::parse_with_params(
        &format!("https://{WEBAPP_HOST}{AUTHORIZE}"),
        &[
            ("redirect_uri", redirect.as_str()),
            ("state", state),
            ("prompt", "select_account"),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("cli_pkce_marker", "1"),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|err| DevinLoginError::Url(err.to_string()))
}

fn session_token_from(value: &serde_json::Value) -> Option<String> {
    for key in [
        "session_token",
        "access_token",
        "api_key",
        "token",
        "windsurf_api_key",
    ] {
        if let Some(token) = value.get(key).and_then(serde_json::Value::as_str)
            && !token.is_empty()
        {
            return Some(token.to_owned());
        }
    }
    for nested in ["session", "tokens", "data", "result"] {
        if let Some(inner) = value.get(nested)
            && let Some(token) = session_token_from(inner)
        {
            return Some(token);
        }
    }
    None
}

async fn exchange_code(code: &str, verifier: &str) -> Result<TokenSet, DevinLoginError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(DevinLoginError::Http)?;
    let response = client
        .post(format!("{DEVIN_API_URL}{TOKEN_EXCHANGE}"))
        .header("content-type", "application/json")
        .json(&json!({ "code": code, "code_verifier": verifier }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(DevinLoginError::Exchange(format!(
            "http {status}: {}",
            body.chars().take(256).collect::<String>()
        )));
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|err| DevinLoginError::Exchange(format!("invalid json: {err}")))?;
    let token = session_token_from(&value).ok_or_else(|| {
        DevinLoginError::Exchange("no session token in exchange response".to_owned())
    })?;
    TokenSet::from_parts(normalize_session_token(&token), None, None, None)
        .map_err(|err| DevinLoginError::Exchange(err.to_string()))
}

pub async fn login(status: &mpsc::Sender<String>) -> Result<TokenSet, DevinLoginError> {
    let (listener, port) = bind_loopback().await?;
    let pkce = Pkce::generate();
    let state = random_state();
    let url = authorize_url(port, &pkce.challenge, &state)?;
    if open::that(&url).is_err() {
        return Err(DevinLoginError::NoBrowser);
    }
    let _ = status
        .send(format!("finish the Devin sign-in in your browser: {url}"))
        .await;
    let response = capture_on(listener, &state).await?;
    exchange_code(&response.code, &pkce.verifier).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_has_pkce_marker() {
        let url = authorize_url(4321, "CHAL", "STATE").unwrap();
        assert!(url.starts_with("https://app.devin.ai/auth/cli/continue?"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A4321%2Fcallback"));
        assert!(url.contains("state=STATE"));
        assert!(url.contains("prompt=select_account"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("cli_pkce_marker=1"));
    }

    #[test]
    fn exchange_response_parses_common_shapes() {
        for body in [
            r#"{"session_token":"devin-session-token$x"}"#,
            r#"{"access_token":"y"}"#,
            r#"{"api_key":"z"}"#,
            r#"{"session":{"session_token":"nested"}}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(body).unwrap();
            assert!(session_token_from(&value).is_some(), "{body}");
        }
        let empty: serde_json::Value = serde_json::from_str("{}").unwrap();
        assert!(session_token_from(&empty).is_none());
    }
}

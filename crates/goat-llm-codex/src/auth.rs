use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use goat_llm::RefreshError;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tiny_http::{Header, Response, Server};

use crate::credential::Credential;

pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub(crate) const ISSUER: &str = "https://auth.openai.com";
pub(crate) const REDIRECT_PORT_PRIMARY: u16 = 1455;
pub(crate) const REDIRECT_PORT_FALLBACK: u16 = 1457;
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub(crate) fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 64];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    Pkce {
        verifier,
        challenge,
    }
}

pub(crate) fn random_nonce() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn authorize_url(challenge: &str, state: &str, redirect_uri: &str) -> String {
    let scope = urlencoding::encode(SCOPE);
    let redirect = urlencoding::encode(redirect_uri);
    format!(
        "{ISSUER}/oauth/authorize?response_type=code\
         &client_id={CLIENT_ID}\
         &redirect_uri={redirect}\
         &scope={scope}\
         &code_challenge={challenge}\
         &code_challenge_method=S256\
         &id_token_add_organizations=true\
         &codex_cli_simplified_flow=true\
         &originator=goat\
         &state={state}"
    )
}

pub(crate) fn bind_listener() -> Result<(Server, u16), String> {
    let primary = format!("127.0.0.1:{REDIRECT_PORT_PRIMARY}");
    if let Ok(listener) = TcpListener::bind(&primary) {
        let server = Server::from_listener(listener, None).map_err(|e| e.to_string())?;
        return Ok((server, REDIRECT_PORT_PRIMARY));
    }
    let fallback = format!("127.0.0.1:{REDIRECT_PORT_FALLBACK}");
    if let Ok(listener) = TcpListener::bind(&fallback) {
        let server = Server::from_listener(listener, None).map_err(|e| e.to_string())?;
        return Ok((server, REDIRECT_PORT_FALLBACK));
    }
    Err(format!(
        "ports {REDIRECT_PORT_PRIMARY}/{REDIRECT_PORT_FALLBACK} occupied"
    ))
}

pub(crate) fn await_callback(server: Server, expected_state: &str) -> Result<String, String> {
    let deadline = std::time::Instant::now() + CALLBACK_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("callback timeout (5 min)".into());
        }
        let req = match server.recv_timeout(remaining) {
            Ok(Some(req)) => req,
            Ok(None) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let url = req.url().to_string();
        let parsed = match url::Url::parse(&format!("http://localhost{url}")) {
            Ok(u) => u,
            Err(e) => {
                let _ = req.respond(plain_response(400, "bad request"));
                return Err(format!("bad callback url: {e}"));
            }
        };
        if parsed.path() != "/auth/callback" {
            let _ = req.respond(plain_response(404, "not found"));
            continue;
        }
        let mut code: Option<String> = None;
        let mut state: Option<String> = None;
        for (k, v) in parsed.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                _ => {}
            }
        }
        if state.as_deref() != Some(expected_state) {
            let _ = req.respond(plain_response(400, "state mismatch"));
            return Err("oauth state mismatch (csrf)".into());
        }
        let Some(code) = code else {
            let _ = req.respond(plain_response(400, "missing code"));
            return Err("oauth response missing code".into());
        };
        let _ = req.respond(plain_response(
            200,
            "Authentication complete. You can close this window.",
        ));
        return Ok(code);
    }
}

fn plain_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = body.to_string();
    let mut resp = Response::from_string(body).with_status_code(status);
    if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..]) {
        resp = resp.with_header(h);
    }
    resp
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

pub(crate) async fn exchange_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    label: Option<String>,
) -> Result<Credential, String> {
    let client = http_client();
    let resp = client
        .post(format!("{ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("client_id", CLIENT_ID),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| format!("transport: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("oauth token exchange failed ({status}): {text}"));
    }
    let token: TokenResponse =
        serde_json::from_str(&text).map_err(|e| format!("bad token response: {e}"))?;
    let account_id = token
        .id_token
        .as_ref()
        .and_then(|jwt| decode_id_token_account(jwt));
    let now_ms = now_ms();
    let expires_at_ms = now_ms + token.expires_in.unwrap_or(3600) * 1000;
    Ok(Credential {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        id_token: token.id_token,
        expires_at_ms,
        last_refresh_ms: now_ms,
        account_id,
        label,
    })
}

pub(crate) async fn refresh_with(
    refresh_token: &str,
    label: Option<String>,
) -> Result<Credential, RefreshError> {
    let client = http_client();
    let resp = client
        .post(format!("{ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|e| RefreshError::Transport(e.to_string()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(RefreshError::Auth(text));
        }
        return Err(RefreshError::Other(format!("{status}: {text}")));
    }
    let token: TokenResponse =
        serde_json::from_str(&text).map_err(|e| RefreshError::Other(e.to_string()))?;
    let account_id = token
        .id_token
        .as_ref()
        .and_then(|jwt| decode_id_token_account(jwt));
    let now_ms = now_ms();
    let expires_at_ms = now_ms + token.expires_in.unwrap_or(3600) * 1000;
    Ok(Credential {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        id_token: token.id_token,
        expires_at_ms,
        last_refresh_ms: now_ms,
        account_id,
        label,
    })
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("codex_cli_rs/0.1.0")
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client")
}

pub(crate) fn decode_id_token_account(jwt: &str) -> Option<String> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(parts[1].as_bytes()).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    claims
        .get("https://api.openai.com/auth")
        .and_then(|v| v.get("chatgpt_account_id"))
        .or_else(|| claims.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub(crate) fn redirect_uri_for(port: u16) -> String {
    format!("http://localhost:{port}/auth/callback")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_and_challenge_round_trip() {
        let p = generate_pkce();
        assert!(p.verifier.len() >= 80);
        let mut hasher = Sha256::new();
        hasher.update(p.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(p.challenge, expected);
    }

    #[test]
    fn id_token_decode_extracts_account_id() {
        // {"chatgpt_account_id":"acct_abc"} as base64url payload
        let payload = serde_json::json!({ "chatgpt_account_id": "acct_abc" }).to_string();
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        let jwt = format!("header.{payload_b64}.signature");
        assert_eq!(decode_id_token_account(&jwt).as_deref(), Some("acct_abc"));
    }

    #[test]
    fn id_token_decode_returns_none_for_garbage() {
        assert!(decode_id_token_account("not-a-jwt").is_none());
    }
}

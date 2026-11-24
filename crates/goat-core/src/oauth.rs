use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub(crate) const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_HEAD_LIMIT: usize = 16 * 1024;
const TERMINAL_REFRESH_ERRORS: &[&str] = &[
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("listen for the sign-in callback")]
    Listen(#[source] std::io::Error),
    #[error("read the sign-in callback")]
    Callback(#[source] std::io::Error),
    #[error("sign-in was denied: {0}")]
    Denied(String),
    #[error("sign-in callback has a mismatched state")]
    StateMismatch,
    #[error("sign-in callback has no code")]
    MissingCode,
    #[error("sign-in callback has no issued client id")]
    MissingClientId,
    #[error("sign-in callback issued a different client id")]
    ClientIdChanged,
    #[error("token request")]
    Http(#[from] reqwest::Error),
    #[error("token endpoint returned {status}: {body}")]
    Token { status: u16, body: String },
    #[error("token response is malformed")]
    Json(#[from] serde_json::Error),
    #[error("token is invalid: {0}")]
    InvalidToken(&'static str),
    #[error("this ChatGPT account did not grant plan usage")]
    PlanNotGranted,
    #[error("sign in again with `goat login`: {0}")]
    Reauthenticate(String),
}

#[derive(Debug)]
pub(crate) struct Pkce {
    pub(crate) verifier: String,
    pub(crate) challenge: String,
}

impl Pkce {
    pub(crate) fn generate() -> Self {
        let verifier = random_token::<64>();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Listener {
    listener: TcpListener,
}

impl Listener {
    pub(crate) async fn bind(ports: &[u16]) -> Result<Self, Error> {
        let mut last = None;
        for port in ports {
            match TcpListener::bind(("127.0.0.1", *port)).await {
                Ok(listener) => return Ok(Self { listener }),
                Err(err) => last = Some(err),
            }
        }
        Err(Error::Listen(last.unwrap_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "no port to listen on")
        })))
    }

    pub(crate) fn redirect_uri(&self) -> Result<String, Error> {
        let port = self.listener.local_addr().map_err(Error::Listen)?.port();
        Ok(format!("http://127.0.0.1:{port}{CALLBACK_PATH}"))
    }

    pub(crate) async fn callback(&self) -> Result<Callback, Error> {
        loop {
            let (mut stream, _) = self.listener.accept().await.map_err(Error::Callback)?;
            let Ok(target) =
                tokio::time::timeout(CALLBACK_READ_TIMEOUT, read_request_target(&mut stream)).await
            else {
                continue;
            };
            let target = target.map_err(Error::Callback)?;
            let Some(callback) = Callback::parse(&target) else {
                respond(&mut stream, "404 Not Found", "").await;
                continue;
            };
            let body = if callback.code.is_some() {
                "<p>goat is signed in. You can close this tab.</p>"
            } else {
                "<p>Sign-in did not complete. Return to goat.</p>"
            };
            respond(&mut stream, "200 OK", body).await;
            return Ok(callback);
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Callback {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) issuer: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) error: Option<String>,
}

impl Callback {
    pub(crate) fn parse(target: &str) -> Option<Self> {
        let url = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
        if url.path() != CALLBACK_PATH {
            return None;
        }
        let mut callback = Self::default();
        for (key, value) in url.query_pairs() {
            let value = Some(value.into_owned());
            match &*key {
                "code" => callback.code = value,
                "state" => callback.state = value,
                "iss" => callback.issuer = value,
                "client_id" => callback.client_id = value,
                "error" => callback.error = value,
                _ => {}
            }
        }
        Some(callback)
    }

    pub(crate) fn into_code(self, state: &str) -> Result<(String, Option<String>), Error> {
        if self.state.as_deref() != Some(state) {
            return Err(Error::StateMismatch);
        }
        if let Some(error) = self.error {
            return Err(Error::Denied(error));
        }
        let code = self.code.ok_or(Error::MissingCode)?;
        Ok((code, self.client_id))
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Issued {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) id_token: Option<String>,
    pub(crate) expires_in: Option<i64>,
    pub(crate) scope: Option<String>,
}

pub(crate) async fn request_token(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<Issued, Error> {
    let response = http.post(url).form(form).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(Error::Token {
            status: status.as_u16(),
            body,
        });
    }
    Ok(serde_json::from_str(&body)?)
}

pub(crate) fn refresh_failure(err: Error) -> Error {
    match err {
        Error::Token { body, .. }
            if TERMINAL_REFRESH_ERRORS
                .iter()
                .any(|code| body.contains(code)) =>
        {
            Error::Reauthenticate(body)
        }
        other => other,
    }
}

pub(crate) fn jwt_claims<T: DeserializeOwned>(token: &str) -> Result<T, Error> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or(Error::InvalidToken("not a jwt"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| Error::InvalidToken("payload is not base64url"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn random_token<const N: usize>() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<N>())
}

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).unwrap_or_else(|err| unreachable!("os random source: {err}"));
    bytes
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

pub(crate) fn authorize_url(base: &str, params: &[(&str, &str)]) -> String {
    Url::parse_with_params(base, params)
        .unwrap_or_else(|err| unreachable!("authorize url is a valid constant: {err}"))
        .to_string()
}

async fn read_request_target(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < REQUEST_HEAD_LIMIT {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or_default();
    Ok(target.to_owned())
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    if stream.write_all(response.as_bytes()).await.is_ok() {
        stream.shutdown().await.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_with_code_and_state_is_parsed() {
        let callback =
            Callback::parse("/auth/callback?code=abc&state=xyz&client_id=oaiapp_1").unwrap();
        assert_eq!(
            callback.into_code("xyz").unwrap(),
            ("abc".to_owned(), Some("oaiapp_1".to_owned()))
        );
    }

    #[test]
    fn callback_with_another_state_is_rejected() {
        let callback = Callback::parse("/auth/callback?code=abc&state=other").unwrap();
        assert!(matches!(
            callback.into_code("xyz"),
            Err(Error::StateMismatch)
        ));
    }

    #[test]
    fn callback_keeps_the_issuer_of_the_authorization_response() {
        let callback =
            Callback::parse("/auth/callback?code=abc&state=xyz&iss=https%3A%2F%2Fas.example")
                .unwrap();
        assert_eq!(callback.issuer.as_deref(), Some("https://as.example"));
    }

    #[test]
    fn callback_on_other_path_is_ignored() {
        assert!(Callback::parse("/favicon.ico").is_none());
    }

    #[test]
    fn pkce_challenge_is_the_hash_of_the_verifier() {
        let pkce = Pkce::generate();
        assert_eq!(
            pkce.challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
        assert!(pkce.verifier.len() >= 43);
    }

    #[test]
    fn terminal_refresh_errors_ask_to_sign_in_again() {
        let err = refresh_failure(Error::Token {
            status: 400,
            body: r#"{"error":"refresh_token_reused"}"#.to_owned(),
        });
        assert!(matches!(err, Error::Reauthenticate(_)));
    }
}

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const REGISTRATION_CLIENT: &str = "dynamic_agent_client";
const CALLBACK_PATH: &str = "/auth/callback";
const PREFERRED_PORT: u16 = 1455;
const REFRESH_MARGIN_SECS: i64 = 120;
const CALLBACK_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
    #[error("id token is invalid: {0}")]
    IdToken(&'static str),
    #[error("this ChatGPT account did not grant plan usage")]
    PlanNotGranted,
    #[error("sign in again: {0}")]
    Reauthenticate(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub client_id: Box<str>,
    pub host_id: Box<str>,
    pub subject: Box<str>,
    pub email: Option<Box<str>>,
    pub id_token: Box<str>,
    pub access_token: Box<str>,
    pub refresh_token: Box<str>,
    pub expires_at: i64,
}

impl Token {
    pub fn expires_soon(&self) -> bool {
        self.expires_at - REFRESH_MARGIN_SECS <= now()
    }

    pub async fn refresh(&self, http: &reqwest::Client) -> Result<Self, Error> {
        let form = [
            ("grant_type", "refresh_token"),
            ("client_id", &*self.client_id),
            ("refresh_token", &*self.refresh_token),
            ("resource", RESOURCE),
        ];
        let issued = request_token(http, &form).await.map_err(|err| match err {
            Error::Token { body, .. } if is_terminal_refresh_error(&body) => {
                Error::Reauthenticate(body)
            }
            other => other,
        })?;
        Ok(Self {
            id_token: issued
                .id_token
                .unwrap_or_else(|| self.id_token.to_string())
                .into(),
            access_token: issued.access_token.into(),
            refresh_token: issued
                .refresh_token
                .unwrap_or_else(|| self.refresh_token.to_string())
                .into(),
            expires_at: now() + issued.expires_in,
            ..self.clone()
        })
    }
}

#[derive(Debug)]
pub struct Authorization {
    listener: TcpListener,
    url: String,
    redirect_uri: String,
    verifier: String,
    state: String,
    nonce: String,
    host_id: Box<str>,
    known_client_id: Option<Box<str>>,
}

impl Authorization {
    pub async fn start(previous: Option<&Token>) -> Result<Self, Error> {
        let listener = bind().await.map_err(Error::Listen)?;
        let port = listener.local_addr().map_err(Error::Listen)?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let verifier = random_token::<32>();
        let state = random_token::<16>();
        let nonce = random_token::<16>();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let host_id = previous.map_or_else(new_host_id, |token| token.host_id.clone());
        let known_client_id = previous.map(|token| token.client_id.clone());

        let mut params = vec![
            (
                "client_id",
                known_client_id.as_deref().unwrap_or(REGISTRATION_CLIENT),
            ),
            ("ext_agent_host_id", &*host_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPE),
            ("resource", RESOURCE),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
        ];
        match previous {
            Some(token) => params.push(("id_token_hint", &token.id_token)),
            None => params.push(("agent_name_hint", "goat")),
        }
        let url = Url::parse_with_params(AUTHORIZE_URL, &params)
            .unwrap_or_else(|err| unreachable!("authorize url is a valid constant: {err}"))
            .to_string();

        Ok(Self {
            listener,
            url,
            redirect_uri,
            verifier,
            state,
            nonce,
            host_id,
            known_client_id,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn finish(self, http: &reqwest::Client) -> Result<Token, Error> {
        let callback = self.wait_for_callback().await?;
        if callback.state.as_deref() != Some(&*self.state) {
            return Err(Error::StateMismatch);
        }
        if let Some(error) = callback.error {
            return Err(Error::Denied(error));
        }
        let code = callback.code.ok_or(Error::MissingCode)?;
        let client_id = match (callback.client_id, self.known_client_id) {
            (Some(issued), Some(known)) if *issued != *known => return Err(Error::ClientIdChanged),
            (Some(issued), _) => issued.into_boxed_str(),
            (None, Some(known)) => known,
            (None, None) => return Err(Error::MissingClientId),
        };

        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", &*client_id),
            ("code", &code),
            ("code_verifier", &self.verifier),
            ("redirect_uri", &self.redirect_uri),
            ("resource", RESOURCE),
        ];
        let issued = request_token(http, &form).await?;
        if !issued
            .scope
            .as_deref()
            .is_some_and(|scope| scope.split(' ').any(|s| s == PLAN_SCOPE))
        {
            return Err(Error::PlanNotGranted);
        }
        let id_token = issued.id_token.ok_or(Error::IdToken("missing"))?;
        let claims = verify_id_token(&id_token, &client_id, &self.nonce)?;
        let refresh_token = issued
            .refresh_token
            .ok_or(Error::IdToken("no refresh token"))?;

        Ok(Token {
            client_id,
            host_id: self.host_id,
            subject: claims.sub.into(),
            email: claims.email.map(String::into_boxed_str),
            id_token: id_token.into(),
            access_token: issued.access_token.into(),
            refresh_token: refresh_token.into(),
            expires_at: now() + issued.expires_in,
        })
    }

    async fn wait_for_callback(&self) -> Result<Callback, Error> {
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
struct Callback {
    code: Option<String>,
    state: Option<String>,
    client_id: Option<String>,
    error: Option<String>,
}

impl Callback {
    fn parse(target: &str) -> Option<Self> {
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
                "client_id" => callback.client_id = value,
                "error" => callback.error = value,
                _ => {}
            }
        }
        Some(callback)
    }
}

#[derive(Deserialize)]
struct Issued {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: i64,
    scope: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    exp: i64,
    nonce: Option<String>,
    sub: String,
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, client_id: &str) -> bool {
        match self {
            Self::One(aud) => aud == client_id,
            Self::Many(auds) => auds.iter().any(|aud| aud == client_id),
        }
    }
}

async fn bind() -> std::io::Result<TcpListener> {
    match TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await {
        Ok(listener) => Ok(listener),
        Err(_) => TcpListener::bind(("127.0.0.1", 0)).await,
    }
}

async fn read_request_target(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
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

async fn request_token(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<Issued, Error> {
    let response = http.post(TOKEN_URL).form(form).send().await?;
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

// OIDC Core 3.1.3.7 allows TLS from the token endpoint to replace signature checks: https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation
fn verify_id_token(id_token: &str, client_id: &str, nonce: &str) -> Result<Claims, Error> {
    let payload = id_token
        .split('.')
        .nth(1)
        .ok_or(Error::IdToken("not a jwt"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| Error::IdToken("payload is not base64url"))?;
    let claims: Claims = serde_json::from_slice(&bytes)?;
    if claims.iss != ISSUER {
        return Err(Error::IdToken("wrong issuer"));
    }
    if !claims.aud.contains(client_id) {
        return Err(Error::IdToken("wrong audience"));
    }
    if claims.exp <= now() {
        return Err(Error::IdToken("expired"));
    }
    if claims.nonce.as_deref() != Some(nonce) {
        return Err(Error::IdToken("wrong nonce"));
    }
    Ok(claims)
}

fn is_terminal_refresh_error(body: &str) -> bool {
    [
        "invalid_grant",
        "invalid_refresh_token",
        "token_expired",
        "refresh_token_expired",
        "refresh_token_invalidated",
        "refresh_token_reused",
    ]
    .iter()
    .any(|code| body.contains(code))
}

fn new_host_id() -> Box<str> {
    let bytes: [u8; 16] = random_bytes();
    let uuid = uuid::Builder::from_random_bytes(bytes).into_uuid();
    format!("urn:uuid:{uuid}").into()
}

fn random_token<const N: usize>() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<N>())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).unwrap_or_else(|err| unreachable!("os random source: {err}"));
    bytes
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_token(claims: &str) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims))
    }

    #[test]
    fn callback_with_code_and_state_is_parsed() {
        let callback =
            Callback::parse("/auth/callback?code=abc&state=xyz&client_id=oaiapp_1").unwrap();
        assert_eq!(callback.code.as_deref(), Some("abc"));
        assert_eq!(callback.state.as_deref(), Some("xyz"));
        assert_eq!(callback.client_id.as_deref(), Some("oaiapp_1"));
    }

    #[test]
    fn callback_on_other_path_is_ignored() {
        assert!(Callback::parse("/favicon.ico").is_none());
    }

    #[test]
    fn valid_id_token_is_accepted() {
        let exp = now() + 600;
        let token = id_token(&format!(
            r#"{{"iss":"https://auth.openai.com","aud":"oaiapp_1","exp":{exp},"nonce":"n","sub":"user-1","email":"a@b.c"}}"#
        ));
        let claims = verify_id_token(&token, "oaiapp_1", "n").unwrap();
        assert_eq!(claims.sub, "user-1");
    }

    #[test]
    fn id_token_with_wrong_nonce_is_rejected() {
        let exp = now() + 600;
        let token = id_token(&format!(
            r#"{{"iss":"https://auth.openai.com","aud":["oaiapp_1"],"exp":{exp},"nonce":"other","sub":"u"}}"#
        ));
        assert!(matches!(
            verify_id_token(&token, "oaiapp_1", "n"),
            Err(Error::IdToken("wrong nonce"))
        ));
    }

    #[test]
    fn host_id_is_a_uuid_urn() {
        assert!(new_host_id().starts_with("urn:uuid:"));
    }
}

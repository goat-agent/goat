pub mod anthropic;
pub mod openai;
mod sse;

use std::time::Duration;

use serde::Deserialize;

use crate::account;
use crate::message::{Message, Origin, Part};
use crate::tool;

const MAX_RETRIES: u32 = 3;
const ERROR_BODY_LIMIT: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request")]
    Http(#[from] reqwest::Error),
    #[error("{status}: {message}")]
    Status { status: u16, message: String },
    #[error("stream: {0}")]
    Stream(String),
    #[error("stream ended before the response completed")]
    Interrupted,
    #[error("malformed response")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub openai: Box<str>,
    pub codex: Box<str>,
    pub anthropic: Box<str>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            openai: openai::BASE_URL.into(),
            codex: openai::CODEX_BASE_URL.into(),
            anthropic: anthropic::BASE_URL.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
}

#[derive(Debug)]
pub struct Request<'a> {
    pub model: &'a str,
    pub instructions: &'a str,
    pub messages: &'a [Message],
    pub trailing: Option<&'a Message>,
    pub tools: &'a [tool::Spec],
    pub use_tools: bool,
    pub origin: &'a Origin,
    pub cache_key: &'a str,
    pub effort: Option<&'a str>,
}

impl<'a> Request<'a> {
    fn all_messages(&self) -> impl Iterator<Item = &'a Message> {
        self.messages.iter().chain(self.trailing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: Box<str>,
    pub efforts: Option<Vec<Box<str>>>,
    pub default_effort: Option<Box<str>>,
}

impl Model {
    pub fn accepts(&self, effort: &str) -> bool {
        self.efforts
            .as_ref()
            .is_none_or(|efforts| efforts.iter().any(|e| &**e == effort))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Chunk<'a> {
    Text(&'a str),
    Reasoning(&'a str),
}

#[derive(Debug)]
pub struct Reply {
    pub parts: Vec<Part>,
    pub usage: Usage,
}

#[derive(Debug)]
pub enum Provider {
    OpenAi(openai::Client),
    Anthropic(anthropic::Client),
}

impl Provider {
    pub fn new(account: &account::Account, http: reqwest::Client, endpoints: &Endpoints) -> Self {
        match (account.kind, &account.credential) {
            (_, account::Credential::Codex(token)) => Self::OpenAi(openai::Client::codex(
                http,
                &endpoints.codex,
                &token.account_id,
            )),
            (account::Kind::OpenAi, _) => {
                Self::OpenAi(openai::Client::new(http, &endpoints.openai))
            }
            (account::Kind::Anthropic, _) => {
                Self::Anthropic(anthropic::Client::new(http, &endpoints.anthropic))
            }
        }
    }

    pub fn default_model(kind: account::Kind) -> &'static str {
        match kind {
            account::Kind::OpenAi => openai::DEFAULT_MODEL,
            account::Kind::Anthropic => anthropic::DEFAULT_MODEL,
        }
    }

    pub async fn send(
        &self,
        request: &Request<'_>,
        bearer: &str,
        on_chunk: &mut dyn FnMut(Chunk<'_>),
    ) -> Result<Reply, Error> {
        match self {
            Self::OpenAi(client) => client.send(request, bearer, on_chunk).await,
            Self::Anthropic(client) => client.send(request, bearer, on_chunk).await,
        }
    }

    pub async fn models(&self, bearer: &str) -> Result<Vec<Model>, Error> {
        match self {
            Self::OpenAi(client) => client.models(bearer).await,
            Self::Anthropic(client) => client.models(bearer).await,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Done,
}

async fn post(build: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
    let mut attempt = 0;
    loop {
        let response = build().send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let retryable = matches!(status.as_u16(), 500 | 502 | 503 | 504 | 529)
            || (status.as_u16() == 429 && retry_after.is_some());
        if !retryable || attempt == MAX_RETRIES {
            return Err(status_error(response).await);
        }
        tokio::time::sleep(Duration::from_secs(retry_after.unwrap_or(1 << attempt))).await;
        attempt += 1;
    }
}

async fn get(request: reqwest::RequestBuilder) -> Result<String, Error> {
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(status_error(response).await);
    }
    Ok(response.text().await?)
}

async fn status_error(response: reqwest::Response) -> Error {
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Error::Status {
        status,
        message: error_message(&body),
    }
}

fn error_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<Detail>,
        detail: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Detail {
        message: Option<String>,
        code: Option<String>,
        r#type: Option<String>,
        resets_at: Option<i64>,
    }

    let raw = || body.chars().take(ERROR_BODY_LIMIT).collect();
    let Ok(envelope) = serde_json::from_str::<Envelope>(body) else {
        return raw();
    };
    if let Some(detail) = envelope.error {
        let reset = detail.resets_at.map_or_else(String::new, resets_in);
        match (detail.code.or(detail.r#type), detail.message) {
            (Some(kind), Some(message)) => return format!("{kind}: {message}{reset}"),
            (None, Some(message)) => return format!("{message}{reset}"),
            (Some(kind), None) => return format!("{kind}{reset}"),
            (None, None) => {}
        }
    }
    match envelope.detail {
        Some(serde_json::Value::String(detail)) => detail,
        Some(detail) => detail.to_string(),
        None => raw(),
    }
}

fn resets_in(at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let minutes = (at - now).max(0) / 60;
    format!(" (resets in {}h {}m)", minutes / 60, minutes % 60)
}

async fn read_events(
    mut response: reqwest::Response,
    mut handle: impl FnMut(&sse::Event) -> Result<Flow, Error>,
) -> Result<(), Error> {
    let mut decoder = sse::Decoder::default();
    while let Some(chunk) = response.chunk().await? {
        decoder.feed(&chunk);
        while let Some(event) = decoder.next_event() {
            if handle(&event)? == Flow::Done {
                return Ok(());
            }
        }
    }
    Err(Error::Interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(efforts: Option<Vec<Box<str>>>) -> Model {
        Model {
            id: "m".into(),
            efforts,
            default_effort: None,
        }
    }

    #[test]
    fn model_with_known_levels_rejects_other_efforts() {
        let model = model(Some(vec!["low".into(), "high".into()]));
        assert!(model.accepts("high"));
        assert!(!model.accepts("ultra"));
    }

    #[test]
    fn model_with_unknown_levels_accepts_any_effort() {
        assert!(model(None).accepts("ultra"));
    }

    #[test]
    fn openai_error_body_shows_code_and_message() {
        let body = r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit reached"}}"#;
        assert_eq!(
            error_message(body),
            "subscription_sharing_usage_limit_exceeded: limit reached"
        );
    }

    #[test]
    fn anthropic_error_body_shows_type_and_message() {
        let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(error_message(body), "overloaded_error: Overloaded");
    }

    #[test]
    fn codex_usage_limit_shows_when_it_resets() {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 2 * 3600
            + 30 * 60
            + 30;
        let body = format!(
            r#"{{"error":{{"type":"usage_limit_reached","plan_type":"pro","resets_at":{at}}}}}"#
        );
        assert_eq!(
            error_message(&body),
            "usage_limit_reached (resets in 2h 30m)"
        );
    }

    #[test]
    fn detail_body_is_shown() {
        assert_eq!(error_message(r#"{"detail":"nope"}"#), "nope");
    }

    #[test]
    fn plain_body_is_shown() {
        assert_eq!(error_message("bad gateway"), "bad gateway");
    }
}

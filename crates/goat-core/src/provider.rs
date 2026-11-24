pub mod anthropic;
pub mod openai;
mod sse;

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::account;
use crate::image;
use crate::message::{Message, Origin, Part};
use crate::tool;

const ERROR_BODY_LIMIT: usize = 2000;
const LONGEST_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request")]
    Http(#[from] reqwest::Error),
    #[error("{status}: {message}")]
    Status {
        status: u16,
        message: String,
        retry_after: Option<Duration>,
    },
    #[error("stream: {0}")]
    Stream(String),
    #[error("busy: {0}")]
    Busy(String),
    #[error("the connection dropped before the reply finished")]
    Interrupted,
    #[error("malformed response")]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub(crate) fn is_transient(&self) -> bool {
        match self {
            Self::Http(err) => err.is_connect() || err.is_timeout() || err.is_request(),
            Self::Status {
                status,
                retry_after,
                ..
            } => {
                matches!(status, 500 | 502 | 503 | 504 | 529)
                    || *status == 429 && retry_after.is_some()
            }
            Self::Busy(_) | Self::Interrupted => true,
            Self::Stream(_) | Self::Json(_) => false,
        }
    }

    pub(crate) fn wait(&self, attempt: u32) -> Duration {
        match self {
            Self::Status {
                retry_after: Some(wait),
                ..
            } => (*wait).min(LONGEST_WAIT),
            _ => Duration::from_secs(1 << attempt.min(5)).min(LONGEST_WAIT),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub openai: Box<str>,
    pub codex: Box<str>,
    pub anthropic: Box<str>,
    pub google_sign_in: Box<str>,
    pub google_token: Box<str>,
    pub gmail: Box<str>,
    pub calendar: Box<str>,
    pub drive: Box<str>,
    pub docs: Box<str>,
    pub sheets: Box<str>,
    pub slides: Box<str>,
    pub people: Box<str>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            openai: openai::BASE_URL.into(),
            codex: openai::CODEX_BASE_URL.into(),
            anthropic: anthropic::BASE_URL.into(),
            google_sign_in: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            google_token: "https://oauth2.googleapis.com/token".into(),
            gmail: "https://gmail.googleapis.com".into(),
            calendar: "https://www.googleapis.com".into(),
            drive: "https://www.googleapis.com".into(),
            docs: "https://docs.googleapis.com".into(),
            sheets: "https://sheets.googleapis.com".into(),
            slides: "https://slides.googleapis.com".into(),
            people: "https://people.googleapis.com".into(),
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
    pub images: &'a Images,
    pub trailing: Option<&'a Message>,
    pub tools: &'a [tool::Spec],
    pub use_tools: bool,
    pub origin: &'a Origin,
    pub cache_key: &'a str,
    pub effort: Option<&'a str>,
}

const OMITTED_IMAGE: &str = "[image omitted]";
const HITS: usize = 8;
const SEARCH_INSTRUCTIONS: &str =
    "Search the web for the user's query and answer in a few sentences, citing the pages you used.";
const BODY_TEXT_BYTES: usize = 256 * 1024;

#[derive(Debug, Default)]
pub struct Images(HashMap<image::Id, String>);

impl Images {
    pub fn insert(&mut self, id: image::Id, base64: String) {
        self.0.insert(id, base64);
    }

    pub fn get(&self, id: &image::Id) -> Option<&str> {
        self.0.get(id).map(String::as_str)
    }

    fn len(&self) -> usize {
        self.0.values().map(String::len).sum()
    }
}

impl<'a> Request<'a> {
    fn all_messages(&self) -> impl Iterator<Item = &'a Message> {
        self.messages.iter().chain(self.trailing)
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
    pub stop: Stop,
    pub limits: Option<Limits>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub minutes: u64,
    pub used_percent: u8,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Finished,
    Truncated,
    Refused,
}

#[derive(Debug, Clone)]
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

    pub async fn search(&self, query: &str, model: &str, bearer: &str) -> Result<Vec<Hit>, Error> {
        let mut hits = match self {
            Self::OpenAi(client) => client.search(query, model, bearer).await?,
            Self::Anthropic(client) => client.search(query, model, bearer).await?,
        };
        let mut seen = Vec::new();
        hits.retain(|hit| {
            let is_new = !seen.contains(&hit.url);
            seen.push(hit.url.clone());
            is_new
        });
        hits.truncate(HITS);
        Ok(hits)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Done,
}

async fn post(request: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
    let response = request.send().await?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(status_error(response).await)
    }
}

fn json(body: &impl serde::Serialize, images: &Images) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(images.len() + BODY_TEXT_BYTES);
    serde_json::to_writer(&mut out, body)?;
    Ok(out)
}

async fn status_error(response: reqwest::Response) -> Error {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response.text().await.unwrap_or_default();
    Error::Status {
        status,
        message: error_message(&body),
        retry_after,
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
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Interrupted)? {
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

    fn status(status: u16, retry_after: Option<u64>) -> Error {
        Error::Status {
            status,
            message: String::new(),
            retry_after: retry_after.map(Duration::from_secs),
        }
    }

    #[test]
    fn only_errors_that_can_pass_are_tried_again() {
        assert!(status(503, None).is_transient());
        assert!(status(529, None).is_transient());
        assert!(status(429, Some(5)).is_transient());
        assert!(!status(429, None).is_transient());
        assert!(!status(400, None).is_transient());
        assert!(!status(401, None).is_transient());
        assert!(Error::Interrupted.is_transient());
        assert!(!Error::Stream("bad".into()).is_transient());
    }

    #[test]
    fn waits_double_up_to_thirty_seconds() {
        let waits: Vec<u64> = (0..7)
            .map(|attempt| Error::Interrupted.wait(attempt).as_secs())
            .collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(status(429, Some(3)).wait(0), Duration::from_secs(3));
        assert_eq!(status(429, Some(600)).wait(0), LONGEST_WAIT);
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

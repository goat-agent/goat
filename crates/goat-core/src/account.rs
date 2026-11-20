pub mod chatgpt;
pub mod codex;
mod oauth;

use serde::{Deserialize, Serialize};

pub use oauth::Error;

crate::id::define!(Id);

const REFRESH_MARGIN_SECS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    OpenAi,
    Anthropic,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Account {
    pub id: Id,
    pub kind: Kind,
    pub name: Box<str>,
    pub credential: Credential,
}

impl Account {
    pub fn new(kind: Kind, name: &str, credential: Credential) -> Self {
        Self {
            id: Id::generate(),
            kind,
            name: name.into(),
            credential,
        }
    }

    pub fn label(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey { key: Box<str> },
    Chatgpt(chatgpt::Token),
    Codex(codex::Token),
}

impl Credential {
    pub fn secret(&self) -> &str {
        match self {
            Self::ApiKey { key } => key,
            Self::Chatgpt(token) => &token.access_token,
            Self::Codex(token) => &token.access_token,
        }
    }

    pub fn expires_at(&self) -> Option<i64> {
        match self {
            Self::ApiKey { .. } => None,
            Self::Chatgpt(token) => Some(token.expires_at),
            Self::Codex(token) => Some(token.expires_at),
        }
    }

    pub fn needs_refresh(&self) -> bool {
        self.expires_at()
            .is_some_and(|at| at - REFRESH_MARGIN_SECS <= oauth::now())
    }

    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.expires_at() > other.expires_at()
    }

    pub async fn refresh(&self, http: &reqwest::Client) -> Result<Self, Error> {
        Ok(match self {
            Self::ApiKey { .. } => self.clone(),
            Self::Chatgpt(token) => Self::Chatgpt(token.refresh(http).await?),
            Self::Codex(token) => Self::Codex(token.refresh(http).await?),
        })
    }
}

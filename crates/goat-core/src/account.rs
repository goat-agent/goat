pub mod chatgpt;

use serde::{Deserialize, Serialize};

crate::id::define!(Id);

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
        format!("{}/{}", self.kind.as_str(), self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey { key: Box<str> },
    Chatgpt(chatgpt::Token),
}

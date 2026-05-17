mod secret_prompt;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::ProviderId;

pub use secret_prompt::SecretPrompt;

/// Default summary for any credential whose envelope holds `{ "api_key", "label" }` —
/// shared by every API-key provider so each crate doesn't re-implement masking.
pub fn summarize_api_key(v: &Value) -> String {
    let key = v.get("api_key").and_then(|x| x.as_str()).unwrap_or("?");
    let label = v.get("label").and_then(|x| x.as_str());
    let masked = mask(key);
    match label {
        Some(l) => format!("{masked}  {l}"),
        None => masked,
    }
}

fn mask(s: &str) -> String {
    if s.len() <= 8 {
        return "***".into();
    }
    format!("{}…{}", &s[..4], &s[s.len() - 4..])
}

#[async_trait]
pub trait Setup: Send + Sync + 'static {
    fn description(&self) -> &str;
    async fn run(&self, ctx: SetupCtx) -> Result<Value, SetupError>;
}

pub struct SetupCtx {
    pub provider: ProviderId,
    pub label: Option<String>,
    pub prompt: Arc<dyn UserPrompt>,
}

pub trait UserPrompt: Send + Sync + 'static {
    fn secret(&self, label: &str, hint: &str) -> Result<String, SetupError>;
    fn info(&self, message: &str);
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl SetupError {
    pub fn other(msg: impl Into<String>) -> Self {
        SetupError::Other(msg.into())
    }
}

use async_trait::async_trait;
use goat_auth::{CredentialKey, CredentialStore};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use thiserror::Error;

const EMBEDDINGS_URL: &str = "https://api.openai.com/v1/embeddings";
const ENV_VAR: &str = "OPENAI_API_KEY";

#[derive(Debug, Error)]
pub enum EmbeddingError {
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    Transport(String),
    #[error("{message}")]
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    #[error("{0}")]
    Provider(String),
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, EmbeddingError>;
}

pub struct OpenAiEmbeddingProvider {
    store: CredentialStore,
    http: reqwest::Client,
}

impl OpenAiEmbeddingProvider {
    pub fn new(store: CredentialStore) -> Self {
        Self {
            store,
            http: reqwest::Client::new(),
        }
    }

    fn api_key(&self) -> Result<String, EmbeddingError> {
        let key = CredentialKey::model("openai", "default");
        let credential = self
            .store
            .resolve(&key, Some(ENV_VAR))
            .ok_or_else(|| EmbeddingError::Auth("no openai credential available".to_owned()))?;
        Ok(credential.bearer().to_owned())
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let api_key = self.api_key()?;
        let body = json!({ "model": model, "input": text });
        let resp = self
            .http
            .post(EMBEDDINGS_URL)
            .bearer_auth(&api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| EmbeddingError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(EmbeddingError::RateLimited {
                    retry_after: None,
                    message: format!("embedding request rate limited ({status}): {detail}"),
                });
            }
            return Err(EmbeddingError::Provider(format!(
                "embedding request failed ({status}): {detail}"
            )));
        }
        let parsed: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| EmbeddingError::Transport(e.to_string()))?;
        parsed
            .data
            .into_iter()
            .next()
            .map(|datum| datum.embedding)
            .ok_or_else(|| EmbeddingError::Provider("empty embedding response".to_owned()))
    }
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
}

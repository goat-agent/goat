use async_trait::async_trait;
use goat_llm::{ApiKeyPool, EmbeddingProvider, LlmError, ProviderId};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::error::{map_error, parse_retry_after};

const EMBEDDINGS_URL: &str = "https://api.openai.com/v1/embeddings";

pub struct OpenAiEmbeddingProvider {
    keys: ApiKeyPool,
    http: reqwest::Client,
}

impl OpenAiEmbeddingProvider {
    pub fn new(keys: ApiKeyPool) -> Self {
        Self {
            keys,
            http: reqwest::Client::new(),
        }
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

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn id(&self) -> ProviderId {
        crate::ID
    }

    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, LlmError> {
        let key = self
            .keys
            .next()
            .ok_or_else(|| LlmError::Auth("no openai keys available".into()))?;
        let body = json!({ "model": model, "input": text });
        let resp = self
            .http
            .post(EMBEDDINGS_URL)
            .bearer_auth(&key.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let retry_after = parse_retry_after(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            if status == StatusCode::TOO_MANY_REQUESTS {
                self.keys.report_rate_limit(&key.api_key, retry_after);
            }
            return Err(map_error(status, retry_after, &text));
        }

        let parsed: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::Provider(format!("decoding embedding response: {e}")))?;
        parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| LlmError::Provider("empty embedding response".into()))
    }
}

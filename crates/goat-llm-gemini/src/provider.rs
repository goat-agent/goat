use async_trait::async_trait;
use eventsource_stream::Eventsource;
use goat_llm::{ApiKeyPool, LlmError, LlmProvider, LlmRequest, LlmStream, ProviderId};
use reqwest::StatusCode;

use crate::body::Body;
use crate::error::{map_error, parse_retry_after};
use crate::stream::translate;

const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

pub struct GeminiProvider {
    keys: ApiKeyPool,
    base: String,
    http: reqwest::Client,
}

impl GeminiProvider {
    pub fn new(keys: ApiKeyPool) -> Self {
        Self {
            keys,
            base: DEFAULT_BASE.to_string(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn id(&self) -> ProviderId {
        crate::ID
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        let key = self
            .keys
            .next()
            .ok_or_else(|| LlmError::Auth("no gemini keys available".into()))?;
        let url = format!(
            "{}/{}:streamGenerateContent?alt=sse",
            self.base,
            req.model.id()
        );
        let body = Body::from(&req);
        let resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", &key.api_key)
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

        let model_id = req.model.id().to_string();
        Ok(translate(resp.bytes_stream().eventsource(), model_id))
    }
}

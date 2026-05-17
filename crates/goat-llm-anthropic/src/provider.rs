use async_trait::async_trait;
use eventsource_stream::Eventsource;
use goat_llm::{ApiKeyPool, LlmError, LlmProvider, LlmRequest, LlmStream, ProviderId};
use reqwest::StatusCode;

use crate::body::Body;
use crate::error::{map_error, parse_retry_after};
use crate::stream::translate;

const VERSION: &str = "2023-06-01";
const URL: &str = "https://api.anthropic.com/v1/messages";

pub struct AnthropicProvider {
    keys: ApiKeyPool,
    http: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(keys: ApiKeyPool) -> Self {
        Self {
            keys,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        crate::ID
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        let key = self
            .keys
            .next()
            .ok_or_else(|| LlmError::Auth("no anthropic keys available".into()))?;
        let body = Body::from(&req);
        let resp = self
            .http
            .post(URL)
            .header("x-api-key", &key.api_key)
            .header("anthropic-version", VERSION)
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

        Ok(translate(resp.bytes_stream().eventsource()))
    }
}

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use goat_llm::{ApiKeyPool, LlmError, LlmProvider, LlmRequest, LlmStream, ProviderId};
use reqwest::StatusCode;

use crate::body::Body;
use crate::error::{map_error, parse_retry_after};
use crate::stream::translate;

const URL: &str = "https://api.moonshot.ai/v1/chat/completions";

pub struct MoonshotProvider {
    keys: ApiKeyPool,
    http: reqwest::Client,
}

impl MoonshotProvider {
    pub fn new(keys: ApiKeyPool) -> Self {
        Self {
            keys,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for MoonshotProvider {
    fn id(&self) -> ProviderId {
        crate::ID
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        let key = self
            .keys
            .next()
            .ok_or_else(|| LlmError::Auth("no moonshot keys available".into()))?;
        let body = Body::from(&req);
        let resp = self
            .http
            .post(URL)
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

        Ok(translate(resp.bytes_stream().eventsource()))
    }
}

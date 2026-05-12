use std::sync::Arc;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use goat_llm::{KeyProvider, LlmError, LlmProvider, LlmRequest, LlmStream, ProviderId};
use reqwest::StatusCode;

use crate::body::Body;
use crate::error::{map_error, parse_retry_after};
use crate::stream::translate;

const URL: &str = "https://open.bigmodel.cn/api/paas/v4/chat/completions";

pub struct ZhipuProvider {
    keys: Arc<dyn KeyProvider>,
    http: reqwest::Client,
}

impl ZhipuProvider {
    pub fn new(keys: Arc<dyn KeyProvider>) -> Self {
        Self {
            keys,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for ZhipuProvider {
    fn id(&self) -> ProviderId {
        crate::ID
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        let key = self
            .keys
            .next(crate::ID)
            .ok_or_else(|| LlmError::Auth("no zhipu keys available".into()))?;
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
                self.keys.report_429(crate::ID, &key.api_key, retry_after);
            }
            return Err(map_error(status, retry_after, &text));
        }

        Ok(translate(resp.bytes_stream().eventsource()))
    }
}

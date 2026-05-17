use std::time::Duration;

use goat_llm::LlmError;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

pub(crate) fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

pub(crate) fn map_error(status: StatusCode, retry_after: Option<Duration>, body: &str) -> LlmError {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return LlmError::Auth(body.to_string());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return LlmError::RateLimited {
            retry_after: retry_after.map(|d| d.as_secs()),
            message: body.to_string(),
        };
    }
    if status.is_client_error() {
        return LlmError::BadRequest(body.to_string());
    }
    LlmError::Provider(format!("{status}: {body}"))
}

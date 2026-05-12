use std::time::Duration;

use goat_llm::{LlmError, StopReason};
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde::Deserialize;

pub(crate) fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: Option<String>,
}

fn extract_message(body: &str) -> String {
    serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|e| e.message)
        .unwrap_or_else(|| body.to_string())
}

pub(crate) fn map_error(status: StatusCode, retry_after: Option<Duration>, body: &str) -> LlmError {
    let message = extract_message(body);
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return LlmError::Auth(message);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return LlmError::RateLimited {
            retry_after: retry_after.map(|d| d.as_secs()),
            message,
        };
    }
    if status.is_client_error() {
        return LlmError::BadRequest(message);
    }
    LlmError::Provider(format!("{status}: {message}"))
}

pub(crate) fn parse_stop(s: &str) -> StopReason {
    match s {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" => StopReason::ToolUse,
        "sensitive" | "network_error" => StopReason::Refused,
        _ => StopReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_message_from_zhipu_envelope() {
        let body = r#"{"code":1001,"message":"quota exceeded"}"#;
        assert_eq!(extract_message(body), "quota exceeded");
    }

    #[test]
    fn falls_back_to_raw_body_on_unknown_shape() {
        let body = "plain text error";
        assert_eq!(extract_message(body), "plain text error");
    }

    #[test]
    fn map_error_uses_extracted_message_on_400() {
        let body = r#"{"code":1002,"message":"bad model"}"#;
        match map_error(StatusCode::BAD_REQUEST, None, body) {
            LlmError::BadRequest(m) => assert_eq!(m, "bad model"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn map_error_429_extracts_message_and_retry() {
        let body = r#"{"code":1003,"message":"too many"}"#;
        match map_error(
            StatusCode::TOO_MANY_REQUESTS,
            Some(Duration::from_secs(5)),
            body,
        ) {
            LlmError::RateLimited {
                retry_after,
                message,
            } => {
                assert_eq!(retry_after, Some(5));
                assert_eq!(message, "too many");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_known_stop_reasons() {
        assert!(matches!(parse_stop("stop"), StopReason::EndTurn));
        assert!(matches!(parse_stop("length"), StopReason::MaxTokens));
        assert!(matches!(parse_stop("tool_calls"), StopReason::ToolUse));
        assert!(matches!(parse_stop("sensitive"), StopReason::Refused));
        assert!(matches!(parse_stop("network_error"), StopReason::Refused));
        assert!(matches!(parse_stop("unknown"), StopReason::Stop));
    }
}

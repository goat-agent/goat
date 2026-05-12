use std::time::Duration;

use goat_llm::{LlmError, StopReason};
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

pub(crate) fn parse_stop(s: &str) -> StopReason {
    match s {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" => StopReason::ToolUse,
        "content_filter" => StopReason::Refused,
        _ => StopReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_basic_status_codes() {
        assert!(matches!(
            map_error(StatusCode::UNAUTHORIZED, None, "x"),
            LlmError::Auth(_)
        ));
        assert!(matches!(
            map_error(StatusCode::BAD_REQUEST, None, "x"),
            LlmError::BadRequest(_)
        ));
        assert!(matches!(
            map_error(StatusCode::INTERNAL_SERVER_ERROR, None, "x"),
            LlmError::Provider(_)
        ));
    }

    #[test]
    fn maps_429_with_retry_after() {
        let e = map_error(
            StatusCode::TOO_MANY_REQUESTS,
            Some(Duration::from_secs(8)),
            "slow",
        );
        match e {
            LlmError::RateLimited { retry_after, .. } => assert_eq!(retry_after, Some(8)),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_known_stop_reasons() {
        assert!(matches!(parse_stop("stop"), StopReason::EndTurn));
        assert!(matches!(parse_stop("length"), StopReason::MaxTokens));
        assert!(matches!(parse_stop("tool_calls"), StopReason::ToolUse));
        assert!(matches!(parse_stop("content_filter"), StopReason::Refused));
        assert!(matches!(parse_stop("unknown"), StopReason::Stop));
    }
}

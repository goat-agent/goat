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
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::Stop,
        "tool_use" => StopReason::ToolUse,
        "refusal" => StopReason::Refused,
        _ => StopReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_401_to_auth() {
        assert!(matches!(
            map_error(StatusCode::UNAUTHORIZED, None, "x"),
            LlmError::Auth(_)
        ));
    }

    #[test]
    fn maps_429_with_retry_after() {
        let e = map_error(
            StatusCode::TOO_MANY_REQUESTS,
            Some(Duration::from_secs(10)),
            "slow",
        );
        match e {
            LlmError::RateLimited { retry_after, .. } => assert_eq!(retry_after, Some(10)),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn maps_400_to_bad_request() {
        assert!(matches!(
            map_error(StatusCode::BAD_REQUEST, None, "x"),
            LlmError::BadRequest(_)
        ));
    }

    #[test]
    fn maps_500_to_provider() {
        assert!(matches!(
            map_error(StatusCode::INTERNAL_SERVER_ERROR, None, "boom"),
            LlmError::Provider(_)
        ));
    }

    #[test]
    fn parses_known_stop_reasons() {
        assert!(matches!(parse_stop("end_turn"), StopReason::EndTurn));
        assert!(matches!(parse_stop("max_tokens"), StopReason::MaxTokens));
        assert!(matches!(parse_stop("tool_use"), StopReason::ToolUse));
        assert!(matches!(parse_stop("refusal"), StopReason::Refused));
        assert!(matches!(parse_stop("unknown"), StopReason::Stop));
    }
}

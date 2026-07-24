use std::time::Duration;

use goat_provider::StreamError;
use serde::Deserialize;

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Default, Deserialize)]
struct ErrorBody {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    message: String,
}

fn parse_body(body: &str) -> ErrorBody {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|envelope| envelope.error)
        .unwrap_or_default()
}

fn overflow_message(message: &str) -> bool {
    message.starts_with("prompt is too long") || message.contains("exceed context limit")
}

pub(crate) fn classify_http(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> StreamError {
    let parsed = parse_body(body);
    let message = if parsed.message.is_empty() {
        format!("{status}: {body}")
    } else {
        parsed.message
    };
    match (status.as_u16(), parsed.kind.as_str()) {
        (429, _) | (_, "rate_limit_error") => StreamError::rate_limited_at(
            message,
            parse_retry_after(headers),
            crate::unified_reset_secs(headers),
        ),
        (529 | 408 | 504, _) | (_, "overloaded_error" | "api_error" | "timeout_error") => {
            StreamError::overloaded(message)
        }
        (401 | 403, _) | (_, "authentication_error" | "permission_error") => {
            StreamError::auth(message)
        }
        (413, _) => StreamError::context_overflow(message),
        (400, _) | (_, "invalid_request_error") => {
            if overflow_message(&message) {
                StreamError::context_overflow(message)
            } else {
                StreamError::invalid_request(message)
            }
        }
        (code, _) if (500..600).contains(&code) => StreamError::overloaded(message),
        (code, _) if (400..500).contains(&code) => StreamError::invalid_request(message),
        _ => StreamError::other(message),
    }
}

pub(crate) fn classify_sse_error(data: &str) -> StreamError {
    let parsed = parse_body(data);
    let message = if parsed.message.is_empty() {
        data.to_owned()
    } else {
        parsed.message
    };
    match parsed.kind.as_str() {
        "rate_limit_error" => StreamError::rate_limited(message, None),
        "overloaded_error" | "api_error" | "timeout_error" => StreamError::overloaded(message),
        "authentication_error" | "permission_error" => StreamError::auth(message),
        "invalid_request_error" => {
            if overflow_message(&message) {
                StreamError::context_overflow(message)
            } else {
                StreamError::invalid_request(message)
            }
        }
        _ => StreamError::other(message),
    }
}

pub(crate) fn transport(err: &reqwest::Error) -> StreamError {
    StreamError::transport(err.to_string())
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let target = parse_http_date_unix(raw)?;
    let delta = target - goat_provider::now_secs();
    Some(Duration::from_secs(u64::try_from(delta).unwrap_or(0)))
}

fn month_number(name: &str) -> Option<i64> {
    match name {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn parse_http_date_unix(s: &str) -> Option<i64> {
    let rest = s.split_once(',').map_or(s, |(_, tail)| tail).trim();
    let mut parts = rest.split_whitespace();
    let day: i64 = parts.next()?.parse().ok()?;
    let month = month_number(parts.next()?)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.splitn(3, ':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    Some(crate::gregorian_to_unix(
        year, month, day, hour, minute, second,
    ))
}

#[cfg(test)]
mod tests {
    use goat_provider::StreamError;

    fn http(status: u16, body: &str) -> StreamError {
        super::classify_http(
            reqwest::StatusCode::from_u16(status).unwrap(),
            &reqwest::header::HeaderMap::new(),
            body,
        )
    }

    #[test]
    fn rate_limit_with_retry_after_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());
        let error = super::classify_http(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests has exceeded your rate limit"}}"#,
        );
        assert_eq!(
            error,
            StreamError::rate_limited(
                "Number of requests has exceeded your rate limit",
                Some(std::time::Duration::from_secs(30)),
            )
        );
    }

    #[test]
    fn rate_limit_carries_unified_reset_of_representative_window() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            "100%".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-5h-reset",
            "1764554400".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-7d-utilization",
            "42%".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-7d-reset",
            "1765000000".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-representative-claim",
            "five_hour".parse().unwrap(),
        );
        let error = super::classify_http(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"limited"}}"#,
        );
        let StreamError::RateLimited { resets_at, .. } = error else {
            panic!("expected rate limited");
        };
        assert_eq!(resets_at, Some(1_764_554_400));
    }

    #[test]
    fn rate_limit_reset_prefers_weekly_when_representative() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            "10%".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-5h-reset",
            "1764554400".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-7d-utilization",
            "100%".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-7d-reset",
            "1765000000".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-representative-claim",
            "seven_day".parse().unwrap(),
        );
        let error = super::classify_http(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"limited"}}"#,
        );
        let StreamError::RateLimited { resets_at, .. } = error else {
            panic!("expected rate limited");
        };
        assert_eq!(resets_at, Some(1_765_000_000));
    }

    #[test]
    fn rate_limit_reset_absent_without_representative_claim() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            "100%".parse().unwrap(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-5h-reset",
            "1764554400".parse().unwrap(),
        );
        let error = super::classify_http(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"limited"}}"#,
        );
        let StreamError::RateLimited { resets_at, .. } = error else {
            panic!("expected rate limited");
        };
        assert_eq!(resets_at, None);
    }

    #[test]
    fn rate_limit_reset_absent_without_unified_headers() {
        let error = http(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"limited"}}"#,
        );
        let StreamError::RateLimited {
            resets_at,
            retry_after,
            ..
        } = error
        else {
            panic!("expected rate limited");
        };
        assert_eq!(resets_at, None);
        assert_eq!(retry_after, None);
    }

    #[test]
    fn overloaded_529() {
        let error = http(
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        assert!(matches!(error, StreamError::Overloaded { .. }));
    }

    #[test]
    fn api_error_500_is_overloaded() {
        let error = http(
            500,
            r#"{"type":"error","error":{"type":"api_error","message":"Internal server error"}}"#,
        );
        assert!(matches!(error, StreamError::Overloaded { .. }));
    }

    #[test]
    fn auth_errors() {
        let error = http(
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
        );
        assert!(matches!(error, StreamError::Auth { .. }));
    }

    #[test]
    fn prompt_too_long_is_overflow() {
        let error = http(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 213413 tokens > 200000 maximum"}}"#,
        );
        assert!(matches!(error, StreamError::ContextOverflow { .. }));
    }

    #[test]
    fn input_plus_max_tokens_is_overflow() {
        let error = http(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"input length and `max_tokens` exceed context limit: 195000 + 16384 > 200000, decrease input length or `max_tokens` and try again"}}"#,
        );
        assert!(matches!(error, StreamError::ContextOverflow { .. }));
    }

    #[test]
    fn request_too_large_is_overflow() {
        let error = http(
            413,
            r#"{"type":"error","error":{"type":"request_too_large","message":"Request body too large"}}"#,
        );
        assert!(matches!(error, StreamError::ContextOverflow { .. }));
    }

    #[test]
    fn plain_400_is_invalid_request() {
        let error = http(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: roles must alternate"}}"#,
        );
        assert!(matches!(error, StreamError::InvalidRequest { .. }));
    }

    #[test]
    fn unparseable_body_keeps_status_context() {
        let error = http(503, "bad gateway");
        assert_eq!(
            error,
            StreamError::overloaded("503 Service Unavailable: bad gateway")
        );
    }

    #[test]
    fn sse_overloaded_mid_stream() {
        let error = super::classify_sse_error(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        assert_eq!(error, StreamError::overloaded("Overloaded"));
    }

    #[test]
    fn sse_timeout_is_retryable() {
        let error = super::classify_sse_error(
            r#"{"type":"error","error":{"type":"timeout_error","message":"timed out"}}"#,
        );
        assert!(matches!(error, StreamError::Overloaded { .. }));
    }

    #[test]
    fn retry_after_http_date() {
        let future = goat_provider::now_secs() + 90;
        let days = future / 86_400;
        let secs = future % 86_400;
        let mut year = 1970;
        let mut remaining = days;
        let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        loop {
            let len = if leap(year) { 366 } else { 365 };
            if remaining < len {
                break;
            }
            remaining -= len;
            year += 1;
        }
        let month_lengths = [
            31,
            if leap(year) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let names = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let mut month = 0;
        while remaining >= month_lengths[month] {
            remaining -= month_lengths[month];
            month += 1;
        }
        let header = format!(
            "Wed, {:02} {} {} {:02}:{:02}:{:02} GMT",
            remaining + 1,
            names[month],
            year,
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60,
        );
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, header.parse().unwrap());
        let error = super::classify_http(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"limited"}}"#,
        );
        let StreamError::RateLimited {
            retry_after: Some(delay),
            ..
        } = error
        else {
            panic!("expected rate limited with retry_after");
        };
        assert!((85..=95).contains(&delay.as_secs()), "got {delay:?}");
    }
}

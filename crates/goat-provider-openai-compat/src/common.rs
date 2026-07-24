use goat_provider::{AuthMethod, Model, StreamError, ValidateError, Validated};
use serde::Deserialize;
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ResponseFailed {
    response: Option<ErrorEnvelope>,
}

#[derive(Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    code: Option<String>,
}

fn parse_error_body(data: &str) -> ErrorBody {
    if let Ok(envelope) = serde_json::from_str::<ErrorEnvelope>(data)
        && let Some(body) = envelope.error
    {
        return body;
    }
    if let Ok(failed) = serde_json::from_str::<ResponseFailed>(data)
        && let Some(body) = failed.response.and_then(|envelope| envelope.error)
    {
        return body;
    }
    serde_json::from_str::<ErrorBody>(data).unwrap_or_default()
}

fn is_overflow_code(code: &str) -> bool {
    matches!(
        code,
        "context_length_exceeded"
            | "context_window_exceeded"
            | "string_above_max_length"
            | "invalid_request_error_context_length"
    )
}

fn overflow_message(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("context length")
        || m.contains("context window")
        || m.contains("context size")
        || m.contains("maximum context")
        || m.contains("reduce the length")
        || m.contains("too many tokens")
        || m.contains("exceeds the available context")
}

fn classify_body(
    body: ErrorBody,
    status: Option<u16>,
    retry_after: Option<std::time::Duration>,
    resets_at: Option<i64>,
    fallback: String,
) -> StreamError {
    let message = if body.message.is_empty() {
        fallback
    } else {
        body.message
    };
    let code = body.code.as_deref().unwrap_or("");
    if is_overflow_code(code) || overflow_message(&message) {
        return StreamError::context_overflow(message);
    }
    if code == "insufficient_quota" {
        return StreamError::other(message);
    }
    if code == "invalid_api_key" || body.kind == "authentication_error" {
        return StreamError::auth(message);
    }
    match (status, code) {
        (Some(429), _) | (_, "rate_limit_exceeded") => {
            StreamError::rate_limited_at(message, retry_after, resets_at)
        }
        (Some(401 | 403), _) => StreamError::auth(message),
        (Some(code), _) if (500..600).contains(&code) => StreamError::overloaded(message),
        (Some(code), _) if (400..500).contains(&code) => StreamError::invalid_request(message),
        (None, _) if body.kind == "server_error" => StreamError::overloaded(message),
        _ => StreamError::other(message),
    }
}

fn parse_go_duration_secs(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut total = 0f64;
    let mut number = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
            continue;
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        match ch {
            'h' => total += value * 3600.0,
            'm' => total += value * 60.0,
            's' => total += value,
            _ => return None,
        }
    }
    if !number.is_empty() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(total.ceil() as u64)
}

fn reset_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let mut soonest: Option<u64> = None;
    for key in ["x-ratelimit-reset-requests", "x-ratelimit-reset-tokens"] {
        if let Some(secs) = headers
            .get(key)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_go_duration_secs)
        {
            soonest = Some(soonest.map_or(secs, |current| current.max(secs)));
        }
    }
    soonest
}

pub fn classify_http(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> StreamError {
    let retry_after = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(std::time::Duration::from_secs);
    let resets_at = reset_after_secs(headers)
        .map(|secs| goat_provider::now_secs() + i64::try_from(secs).unwrap_or(i64::MAX));
    classify_body(
        parse_error_body(body),
        Some(status.as_u16()),
        retry_after,
        resets_at,
        format!("{status}: {body}"),
    )
}

pub(crate) fn classify_stream_error(data: &str) -> StreamError {
    classify_body(parse_error_body(data), None, None, None, data.to_owned())
}

pub fn with_snapshot_reset(
    error: StreamError,
    snapshot: Option<&goat_provider::RateLimitSnapshot>,
) -> StreamError {
    let StreamError::RateLimited {
        retry_after,
        resets_at: None,
        message,
    } = error
    else {
        return error;
    };
    let now = goat_provider::now_secs();
    let soonest = snapshot
        .map(|snapshot| snapshot.windows.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|window| window.resets_at)
        .filter(|ts| *ts > now)
        .max();
    StreamError::rate_limited_at(message, retry_after, soonest)
}

pub fn transport(err: &reqwest::Error) -> StreamError {
    StreamError::transport(err.to_string())
}

pub fn tool_arguments(input: &serde_json::Value) -> String {
    if input.is_object() {
        input.to_string()
    } else {
        "{}".to_owned()
    }
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_mins(5))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

pub fn authenticated(auth: AuthMethod, bearer: &Option<String>) -> bool {
    match auth {
        AuthMethod::None => true,
        _ => bearer.is_some(),
    }
}

pub fn validate_bearer(
    client: reqwest::Client,
    url: String,
    auth: AuthMethod,
    bearer: Option<String>,
) -> JoinHandle<Result<Validated, ValidateError>> {
    tokio::spawn(async move {
        if matches!(auth, AuthMethod::None) {
            return Ok(Validated::Assumed);
        }
        let Some(token) = bearer else {
            return Err(ValidateError::NoCredentials);
        };
        let resp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| ValidateError::unreachable("could not reach provider"))?;
        let status = resp.status();
        if status.is_success() {
            Ok(Validated::Live)
        } else if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            Err(ValidateError::InvalidCredentials)
        } else {
            Err(ValidateError::unreachable(format!(
                "could not reach provider: {status}"
            )))
        }
    })
}

pub fn discover_models(
    client: reqwest::Client,
    url: String,
    bearer: Option<String>,
    filter: Option<fn(&str) -> bool>,
    vision_filter: fn(&str) -> bool,
    tx: mpsc::Sender<Model>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut builder = client.get(&url);
        if let Some(token) = &bearer {
            builder = builder.bearer_auth(token);
        }
        let Ok(resp) = builder.send().await else {
            return;
        };
        let Ok(models) = resp.json::<ModelsResponse>().await else {
            return;
        };
        for model in models.data {
            if let Some(keep) = filter
                && !keep(&model.id)
            {
                continue;
            }
            let supports_images = vision_filter(&model.id);
            if tx
                .send(Model {
                    id: model.id,
                    supports_images,
                })
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

#[derive(Deserialize)]
pub(crate) struct ModelsResponse {
    #[serde(default)]
    pub data: Vec<ModelDto>,
}

#[derive(Deserialize)]
pub(crate) struct ModelDto {
    pub id: String,
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
    fn context_length_exceeded_code() {
        let error = http(
            400,
            r#"{"error":{"message":"This model's maximum context length is 128000 tokens.","type":"invalid_request_error","code":"context_length_exceeded"}}"#,
        );
        assert!(matches!(error, StreamError::ContextOverflow { .. }));
    }

    #[test]
    fn non_openai_overflow_wordings_are_context_overflow() {
        for body in [
            r#"{"error":{"message":"the request exceeds the available context size","type":"invalid_request_error"}}"#,
            r#"{"error":{"message":"This model's maximum context is 32768 tokens","code":"string_above_max_length"}}"#,
            r#"{"error":{"message":"Please reduce the length of the messages"}}"#,
            r#"{"error":{"message":"Input is too many tokens for this model"}}"#,
        ] {
            assert!(
                matches!(http(400, body), StreamError::ContextOverflow { .. }),
                "expected overflow for: {body}"
            );
        }
    }

    #[test]
    fn rate_limit_with_retry_after() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "12".parse().unwrap());
        let error = super::classify_http(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"error":{"message":"Rate limit reached","type":"requests","code":"rate_limit_exceeded"}}"#,
        );
        assert_eq!(
            error,
            StreamError::rate_limited(
                "Rate limit reached",
                Some(std::time::Duration::from_secs(12)),
            )
        );
    }

    #[test]
    fn rate_limit_reset_header_becomes_absolute_resets_at() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-reset-requests", "6m0s".parse().unwrap());
        headers.insert("x-ratelimit-reset-tokens", "7.66s".parse().unwrap());
        let before = goat_provider::now_secs();
        let error = super::classify_http(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"error":{"message":"Rate limit reached","code":"rate_limit_exceeded"}}"#,
        );
        let StreamError::RateLimited { resets_at, .. } = error else {
            panic!("expected rate limited");
        };
        let ts = resets_at.expect("resets_at present");
        assert!(ts >= before + 360, "expected >= now+360, got {ts}");
        assert!(ts <= goat_provider::now_secs() + 361, "got {ts}");
    }

    #[test]
    fn rate_limit_without_reset_headers_has_no_resets_at() {
        let error = http(
            429,
            r#"{"error":{"message":"Rate limit reached","code":"rate_limit_exceeded"}}"#,
        );
        let StreamError::RateLimited { resets_at, .. } = error else {
            panic!("expected rate limited");
        };
        assert_eq!(resets_at, None);
    }

    #[test]
    fn go_duration_parses_mixed_units() {
        assert_eq!(super::parse_go_duration_secs("1s"), Some(1));
        assert_eq!(super::parse_go_duration_secs("7.66s"), Some(8));
        assert_eq!(super::parse_go_duration_secs("2m59.56s"), Some(180));
        assert_eq!(super::parse_go_duration_secs("6m0s"), Some(360));
        assert_eq!(super::parse_go_duration_secs("1h2m3s"), Some(3723));
        assert_eq!(super::parse_go_duration_secs("garbage"), None);
        assert_eq!(super::parse_go_duration_secs(""), None);
    }

    #[test]
    fn insufficient_quota_is_other() {
        let error = http(
            429,
            r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
        );
        assert!(matches!(error, StreamError::Other { .. }));
    }

    #[test]
    fn invalid_api_key_is_auth() {
        let error = http(
            401,
            r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error","code":"invalid_api_key"}}"#,
        );
        assert!(matches!(error, StreamError::Auth { .. }));
    }

    #[test]
    fn server_errors_are_overloaded() {
        let error = http(
            503,
            r#"{"error":{"message":"The server is overloaded","type":"server_error"}}"#,
        );
        assert!(matches!(error, StreamError::Overloaded { .. }));
    }

    #[test]
    fn response_failed_envelope() {
        let error = super::classify_stream_error(
            r#"{"response":{"error":{"code":"rate_limit_exceeded","message":"slow down"}}}"#,
        );
        assert_eq!(error, StreamError::rate_limited("slow down", None));
    }

    #[test]
    fn unparseable_keeps_context() {
        let error = http(502, "bad gateway");
        assert_eq!(
            error,
            StreamError::overloaded("502 Bad Gateway: bad gateway")
        );
    }
}

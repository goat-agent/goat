use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("auth: {0}")]
    Auth(String),
    #[error("rate-limited (retry_after={retry_after:?}s): {message}")]
    RateLimited {
        retry_after: Option<u64>,
        message: String,
    },
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("provider error: {0}")]
    Provider(String),
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("model must be `<provider>/<id>`, got `{0}`")]
    BadFormat(String),
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
}

/// How many *consecutive* malformed SSE frames a provider stream tolerates
/// before treating the stream as structurally broken.
///
/// A handful of unknown or heartbeat frames are normal and should be skipped,
/// but a sustained run of parse failures means real content is being silently
/// dropped — surfacing an error is better than truncating the response without
/// any signal. Each provider's stream loop owns the per-provider wire types
/// (the parsing quirk) but shares this tolerance policy.
pub const MAX_CONSECUTIVE_SSE_PARSE_FAILURES: u32 = 8;

/// Returns `Some(err)` once `consecutive_failures` reaches
/// [`MAX_CONSECUTIVE_SSE_PARSE_FAILURES`], signalling the stream loop to yield
/// the error and stop rather than skip yet another frame. Returns `None` while
/// the failure run is still within tolerance (the caller logs and continues).
pub fn sse_parse_failure_limit(consecutive_failures: u32) -> Option<LlmError> {
    if consecutive_failures >= MAX_CONSECUTIVE_SSE_PARSE_FAILURES {
        Some(LlmError::Provider(format!(
            "stream aborted after {consecutive_failures} consecutive unparseable SSE frames"
        )))
    } else {
        None
    }
}

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

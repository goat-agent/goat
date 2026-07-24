use goat_tool::ToolError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum WebFetchError {
    #[error("could not build HTTP client: {0}")]
    Client(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error("server returned {0}")]
    Status(u16),
    #[error("download failed: {0}")]
    Download(String),
    #[error("refusing to fetch a private or local address: {0}")]
    Blocked(String),
    #[error("headless render failed: {0}")]
    Render(String),
}

impl From<WebFetchError> for ToolError {
    fn from(err: WebFetchError) -> Self {
        ToolError::Execution {
            message: err.to_string(),
        }
    }
}

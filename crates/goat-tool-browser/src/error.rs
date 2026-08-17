#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("{0}")]
    Input(String),
    #[error("{0}")]
    Message(String),
    #[error("{op} timed out after {ms} ms")]
    Timeout { op: &'static str, ms: u128 },
}

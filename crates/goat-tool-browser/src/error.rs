use chromiumoxide::error::CdpError;

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("{0}")]
    Input(String),
    #[error("{0}")]
    Message(String),
    #[error("{op} timed out after {ms} ms")]
    Timeout { op: &'static str, ms: u128 },
    #[error("Chrome not found; install Google Chrome to use the browser tool ({0})")]
    NoChrome(String),
    #[error("could not resolve ~/.goat/browser/profile")]
    NoProfile,
}

impl From<CdpError> for BrowserError {
    fn from(err: CdpError) -> Self {
        match err {
            CdpError::JavascriptException(details) => {
                Self::Message(format!("javascript exception: {}", details.text))
            }
            CdpError::Timeout => Self::Message("the browser command timed out".to_owned()),
            CdpError::NotFound => {
                Self::Message("element not found; take a new snapshot".to_owned())
            }
            other => Self::Message(other.to_string()),
        }
    }
}

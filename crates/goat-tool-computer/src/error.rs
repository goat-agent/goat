#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    #[error("invalid action input: {0}")]
    InvalidInput(String),
    #[error("computer use unavailable: {0}")]
    Unavailable(String),
    #[error("no monitor found")]
    NoMonitor,
    #[error("unknown key: {0}")]
    UnknownKey(String),
    #[cfg(not(target_os = "linux"))]
    #[error("screen capture failed: {0}")]
    Capture(#[from] xcap::XCapError),
    #[cfg(not(target_os = "linux"))]
    #[error("input connection failed: {0}")]
    Connection(#[from] enigo::NewConError),
    #[cfg(not(target_os = "linux"))]
    #[error("input simulation failed: {0}")]
    Input(#[from] enigo::InputError),
    #[cfg(not(target_os = "linux"))]
    #[error("image encode failed: {0}")]
    Encode(#[from] image::ImageError),
}

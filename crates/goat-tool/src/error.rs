#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolErrorClass {
    InvalidInput,
    Policy,
    NotFound,
    Io,
    Timeout,
    Execution,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    class: ToolErrorClass,
    message: String,
}

impl ToolError {
    pub fn new(class: ToolErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ToolErrorClass::InvalidInput, message)
    }

    pub fn policy(message: impl Into<String>) -> Self {
        Self::new(ToolErrorClass::Policy, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ToolErrorClass::NotFound, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ToolErrorClass::Io, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ToolErrorClass::Timeout, message)
    }

    pub fn execution(message: impl Into<String>) -> Self {
        Self::new(ToolErrorClass::Execution, message)
    }

    pub fn class(&self) -> ToolErrorClass {
        self.class
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(source: serde_json::Error) -> Self {
        Self::invalid_input(format!("invalid tool input: {source}"))
    }
}

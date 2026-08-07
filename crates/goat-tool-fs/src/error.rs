#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("file not found: {path}")]
    NotFound { path: String },
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("no match for old_string in {path}")]
    NoMatch { path: String },
    #[error("old_string is not unique in {path}; add more context")]
    NotUnique { path: String },
}

impl From<FsError> for goat_tool::ToolError {
    fn from(error: FsError) -> Self {
        let class = match error {
            FsError::NotFound { .. } => goat_tool::ToolErrorClass::NotFound,
            FsError::Io { .. } => goat_tool::ToolErrorClass::Io,
            FsError::NoMatch { .. } | FsError::NotUnique { .. } => {
                goat_tool::ToolErrorClass::Execution
            }
        };
        goat_tool::ToolError::new(class, error.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },
    #[error("unknown skill: {name}")]
    UnknownSkill { name: String },
    #[error("invalid tool input: {source}")]
    InvalidInput {
        #[from]
        source: serde_json::Error,
    },
    #[error("path escapes the session directory: {path}")]
    PathEscape { path: String },
    #[error("path is outside the active workspace: {path}")]
    PathBlocked { path: String },
    #[error("writes are restricted to the plan file while planning: {path}")]
    WriteBlocked { path: String },
    #[error("file not found: {path}")]
    NotFound { path: String },
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("no match for old_string in {path}")]
    EditNoMatch { path: String },
    #[error("old_string is not unique in {path}; add more context")]
    EditNotUnique { path: String },
    #[error("invalid regex: {source}")]
    Regex {
        #[from]
        source: regex::Error,
    },
    #[error("command timed out after {ms}ms")]
    Timeout { ms: u64 },
    #[error("failed to spawn command: {source}")]
    Spawn { source: std::io::Error },
    #[error("could not resolve working directory: {source}")]
    Cwd { source: std::io::Error },
    #[error("{message}")]
    Execution { message: String },
}

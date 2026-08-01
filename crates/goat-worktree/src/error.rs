use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error(transparent)]
    Git(#[from] goat_git::GitError),
    #[error("invalid worktree label '{label}': {reason}")]
    InvalidLabel { label: String, reason: &'static str },
    #[error("failed to get current directory: {source}")]
    CurrentDir { source: std::io::Error },
    #[error("failed to enter worktree {path}: {source}")]
    Enter {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("worktree path already exists and is not a managed worktree: {path}")]
    PathCollision { path: PathBuf },
    #[error("worktree path belongs to a different git repository: {path}")]
    WrongRepository { path: PathBuf },
    #[error("branch {branch} is already checked out at {path}")]
    BranchCheckedOut { branch: String, path: PathBuf },
    #[error("unknown managed worktree: {label}")]
    UnknownWorktree { label: String },
    #[error("worktree {label} has uncommitted changes or untracked files")]
    DirtyWorktree { label: String },
    #[error("worktree {label} has commits only on {branch}")]
    UniqueCommits { label: String, branch: String },
    #[error("invalid .worktreeinclude at {path}: {message}")]
    IgnorePattern { path: PathBuf, message: String },
    #[error("invalid worktree metadata: {0}")]
    Json(serde_json::Error),
}

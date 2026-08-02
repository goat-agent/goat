mod intent;
mod refs;
mod run;
mod snapshot;

pub use intent::{GitOp, GitVerb, classify};
pub use refs::{
    Worktree, branch_exists, commit_exists, commit_oid, common_dir, git_dir, head_branch, is_dirty,
    parse_head, parse_worktrees, repo_root, validate_branch_name, worktrees,
};
pub use run::{Capture, GitError, Output, capture, output, path_arg};
pub use snapshot::{Snapshot, head_subject, snapshot};

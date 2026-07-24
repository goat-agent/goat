use std::path::{Component, Path, PathBuf};

use crate::error::WorktreeError;
use crate::git::{git_output, git_worktrees, os, repo_root};

const GOAT_DIR: &str = ".goat";
const WORKTREES_DIR: &str = "worktrees";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub owner_root: PathBuf,
    pub repo_root: PathBuf,
    pub git_branch: String,
    pub kind: WorkspaceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceKind {
    Main,
    Managed { label: String },
    OtherWorktree,
}

pub fn workspace(cwd: &Path) -> Result<Workspace, WorktreeError> {
    let repo_root = repo_root(cwd)?;
    let worktrees = git_worktrees(&repo_root)?;
    let owner_root = owner_root(&repo_root, &worktrees);
    let bucket = owner_root.join(GOAT_DIR).join(WORKTREES_DIR);
    let kind = if let Some(label) = managed_label(cwd, &bucket) {
        WorkspaceKind::Managed { label }
    } else if repo_root != owner_root {
        WorkspaceKind::OtherWorktree
    } else {
        WorkspaceKind::Main
    };
    let mut git_branch = current_branch(cwd)?;
    if git_branch.is_empty() {
        git_branch = short_head(cwd)?;
    }
    Ok(Workspace {
        owner_root,
        repo_root,
        git_branch,
        kind,
    })
}

impl Workspace {
    pub fn head_branch(&self) -> Option<String> {
        let head = self.repo_root.join(".git").join("HEAD");
        parse_head(&std::fs::read_to_string(head).ok()?)
    }
}

fn parse_head(content: &str) -> Option<String> {
    let content = content.trim();
    if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_owned());
    }
    if !content.is_empty() && content.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(content.chars().take(7).collect());
    }
    None
}

fn owner_root(current_root: &Path, worktrees: &[crate::git::GitWorktree]) -> PathBuf {
    for worktree in worktrees {
        let bucket = worktree.path.join(GOAT_DIR).join(WORKTREES_DIR);
        if current_root.starts_with(&bucket) {
            return worktree.path.clone();
        }
    }
    current_root.to_path_buf()
}

fn managed_label(cwd: &Path, bucket: &Path) -> Option<String> {
    let cwd = cwd.canonicalize().ok()?;
    let bucket = bucket.canonicalize().ok()?;
    if !cwd.starts_with(&bucket) {
        return None;
    }
    let rel = cwd.strip_prefix(&bucket).ok()?;
    let label = match rel.components().next() {
        Some(Component::Normal(label)) => label.to_string_lossy().into_owned(),
        _ => return None,
    };
    if label.starts_with('.') {
        return None;
    }
    Some(label)
}

fn current_branch(cwd: &Path) -> Result<String, WorktreeError> {
    let output = git_output(cwd, &[os("branch"), os("--show-current")])?;
    Ok(output.stdout.trim().to_owned())
}

fn short_head(cwd: &Path) -> Result<String, WorktreeError> {
    let output = git_output(cwd, &[os("rev-parse"), os("--short"), os("HEAD")])?;
    Ok(output.stdout.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::parse_worktrees;

    #[test]
    fn owner_root_from_managed_path() {
        let input = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo/.goat/worktrees/plan\nHEAD def\nbranch refs/heads/worktree-plan\n\n";
        let worktrees = parse_worktrees(input);
        let plan = PathBuf::from("/repo/.goat/worktrees/plan");
        assert_eq!(owner_root(&plan, &worktrees), PathBuf::from("/repo"));
    }

    #[test]
    fn parse_head_symbolic_and_detached() {
        assert_eq!(
            parse_head("ref: refs/heads/main\n").as_deref(),
            Some("main")
        );
        assert_eq!(
            parse_head("ref: refs/heads/feature/foo\n").as_deref(),
            Some("feature/foo")
        );
        assert_eq!(
            parse_head("5894a7d0deadbeef1234\n").as_deref(),
            Some("5894a7d")
        );
        assert_eq!(parse_head("\n"), None);
    }
}

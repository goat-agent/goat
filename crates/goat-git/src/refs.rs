use std::path::{Path, PathBuf};

use crate::run::{GitError, capture, output};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

pub fn repo_root(cwd: &Path) -> Result<PathBuf, GitError> {
    let capture = capture(cwd, &["rev-parse", "--show-toplevel"])?;
    if !capture.status.success() {
        return Err(GitError::NotARepository);
    }
    let raw = capture.stdout.trim();
    if raw.is_empty() {
        return Err(GitError::NotARepository);
    }
    PathBuf::from(raw)
        .canonicalize()
        .map_err(|source| GitError::Io {
            path: PathBuf::from(raw),
            source,
        })
}

pub fn common_dir(root: &Path) -> Result<PathBuf, GitError> {
    let output = output(root, &["rev-parse", "--git-common-dir"])?;
    let raw = output.stdout.trim();
    if raw.is_empty() {
        return Err(GitError::NotARepository);
    }
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    path.canonicalize()
        .map_err(|source| GitError::Io { path, source })
}

pub fn git_dir(cwd: &Path) -> Result<PathBuf, GitError> {
    let output = output(cwd, &["rev-parse", "--absolute-git-dir"])?;
    let raw = output.stdout.trim();
    if raw.is_empty() {
        return Err(GitError::NotARepository);
    }
    Ok(PathBuf::from(raw))
}

pub fn worktrees(root: &Path) -> Result<Vec<Worktree>, GitError> {
    let output = output(root, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktrees(&output.stdout))
}

pub fn parse_worktrees(input: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    for line in input.lines() {
        if line.is_empty() {
            if let Some(path) = path.take() {
                out.push(Worktree {
                    path,
                    branch: branch.take(),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("worktree ") {
            if let Some(path) = path.replace(PathBuf::from(value)) {
                out.push(Worktree {
                    path,
                    branch: branch.take(),
                });
            }
        } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
            branch = Some(value.to_owned());
        }
    }
    if let Some(path) = path {
        out.push(Worktree { path, branch });
    }
    for worktree in &mut out {
        if let Ok(canonical) = worktree.path.canonicalize() {
            worktree.path = canonical;
        }
    }
    out
}

pub fn validate_branch_name(root: &Path, branch: &str) -> Result<(), GitError> {
    output(root, &["check-ref-format", "--branch", branch])?;
    Ok(())
}

pub fn branch_exists(root: &Path, branch: &str) -> Result<bool, GitError> {
    let reference = format!("refs/heads/{branch}");
    let capture = capture(root, &["show-ref", "--verify", "--quiet", &reference])?;
    match capture.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(capture.into_failure()),
    }
}

pub fn commit_exists(root: &Path, reference: &str) -> Result<bool, GitError> {
    let spec = format!("{reference}^{{commit}}");
    let capture = capture(root, &["rev-parse", "--verify", "--quiet", &spec])?;
    match capture.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(capture.into_failure()),
    }
}

pub fn commit_oid(root: &Path, reference: &str) -> Result<String, GitError> {
    let spec = format!("{reference}^{{commit}}");
    let output = output(root, &["rev-parse", "--verify", &spec])?;
    Ok(output.stdout.trim().to_owned())
}

pub fn is_dirty(path: &Path) -> Result<bool, GitError> {
    let output = output(
        path,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    Ok(!output.stdout.trim().is_empty())
}

pub fn head_branch(git_dir: &Path) -> Option<String> {
    parse_head(&std::fs::read_to_string(git_dir.join("HEAD")).ok()?)
}

pub fn parse_head(content: &str) -> Option<String> {
    let content = content.trim();
    if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_owned());
    }
    if !content.is_empty() && content.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(content.chars().take(7).collect());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{parse_head, parse_worktrees};

    #[test]
    fn parses_worktree_porcelain() {
        let input = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo/.goat/worktrees/plan\nHEAD def\nbranch refs/heads/worktree-plan\n\n";
        let parsed = parse_worktrees(input);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].branch.as_deref(), Some("main"));
        assert_eq!(parsed[1].branch.as_deref(), Some("worktree-plan"));
    }

    #[test]
    fn parses_head_ref_and_detached() {
        assert_eq!(
            parse_head("ref: refs/heads/feature-x\n").as_deref(),
            Some("feature-x")
        );
        assert_eq!(
            parse_head("05808dd06e679c9f7727d04a9a73760f198be0b8\n").as_deref(),
            Some("05808dd")
        );
        assert_eq!(parse_head("\n"), None);
    }
}

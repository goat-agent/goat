use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use crate::error::WorktreeError;

pub(crate) struct GitOutput {
    pub(crate) stdout: String,
}

pub(crate) struct GitStatus {
    pub(crate) command: String,
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Clone)]
pub(crate) struct GitWorktree {
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<String>,
}

#[derive(Clone)]
pub(crate) struct BaseRef {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) oid: String,
}

pub(crate) enum ExistingBase {
    Branch(String),
    Ref(BaseRef),
}

pub(crate) fn git_output(cwd: &Path, args: &[OsString]) -> Result<GitOutput, WorktreeError> {
    let status = git_status(cwd, args)?;
    if status.status.success() {
        Ok(GitOutput {
            stdout: status.stdout,
        })
    } else {
        Err(WorktreeError::GitFailed {
            command: status.command,
            status: status.status.code(),
            stdout: status.stdout,
            stderr: status.stderr,
        })
    }
}

pub(crate) fn git_status(cwd: &Path, args: &[OsString]) -> Result<GitStatus, WorktreeError> {
    let command = format_command(args);
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                WorktreeError::GitMissing
            } else {
                WorktreeError::Spawn { source }
            }
        })?;
    Ok(GitStatus {
        command,
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn format_command(args: &[OsString]) -> String {
    let mut parts = vec!["git".to_owned()];
    parts.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
    parts.join(" ")
}

pub(crate) fn os(value: &str) -> OsString {
    OsString::from(value)
}

#[cfg(windows)]
pub(crate) fn git_path(path: &Path) -> OsString {
    let value = path.to_string_lossy();
    let value = if let Some(stripped) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{stripped}")
    } else if let Some(stripped) = value.strip_prefix(r"\\?\") {
        stripped.to_owned()
    } else {
        value.into_owned()
    };
    value.replace('\\', "/").into()
}

#[cfg(not(windows))]
pub(crate) fn git_path(path: &Path) -> OsString {
    path.as_os_str().to_os_string()
}

pub(crate) fn repo_root(cwd: &Path) -> Result<PathBuf, WorktreeError> {
    let status = git_status(cwd, &[os("rev-parse"), os("--show-toplevel")])?;
    if !status.status.success() {
        return Err(WorktreeError::NotGitRepository);
    }
    let raw = status.stdout.trim();
    if raw.is_empty() {
        return Err(WorktreeError::NotGitRepository);
    }
    PathBuf::from(raw)
        .canonicalize()
        .map_err(|source| WorktreeError::Io {
            path: PathBuf::from(raw),
            source,
        })
}

pub(crate) fn common_dir(root: &Path) -> Result<PathBuf, WorktreeError> {
    let output = git_output(root, &[os("rev-parse"), os("--git-common-dir")])?;
    let raw = output.stdout.trim();
    if raw.is_empty() {
        return Err(WorktreeError::NotGitRepository);
    }
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    path.canonicalize()
        .map_err(|source| WorktreeError::Io { path, source })
}

pub(crate) fn git_worktrees(root: &Path) -> Result<Vec<GitWorktree>, WorktreeError> {
    let output = git_output(root, &[os("worktree"), os("list"), os("--porcelain")])?;
    Ok(parse_worktrees(&output.stdout))
}

pub(crate) fn parse_worktrees(input: &str) -> Vec<GitWorktree> {
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    for line in input.lines() {
        if line.is_empty() {
            if let Some(path) = path.take() {
                out.push(GitWorktree {
                    path,
                    branch: branch.take(),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("worktree ") {
            if let Some(path) = path.replace(PathBuf::from(value)) {
                out.push(GitWorktree {
                    path,
                    branch: branch.take(),
                });
            }
        } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
            branch = Some(value.to_owned());
        }
    }
    if let Some(path) = path {
        out.push(GitWorktree { path, branch });
    }
    for worktree in &mut out {
        if let Ok(canonical) = worktree.path.canonicalize() {
            worktree.path = canonical;
        }
    }
    out
}

pub(crate) fn validate_branch_name(root: &Path, branch: &str) -> Result<(), WorktreeError> {
    git_output(root, &[os("check-ref-format"), os("--branch"), os(branch)])?;
    Ok(())
}

pub(crate) fn branch_exists(root: &Path, branch: &str) -> Result<bool, WorktreeError> {
    let status = git_status(
        root,
        &[
            os("show-ref"),
            os("--verify"),
            os("--quiet"),
            os(&format!("refs/heads/{branch}")),
        ],
    )?;
    match status.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(WorktreeError::GitFailed {
            command: status.command,
            status: status.status.code(),
            stdout: status.stdout,
            stderr: status.stderr,
        }),
    }
}

pub(crate) fn resolve_base_ref(root: &Path) -> Result<BaseRef, WorktreeError> {
    if commit_exists(root, "origin/HEAD")? {
        return Ok(BaseRef {
            name: "origin/HEAD".to_owned(),
            kind: "origin_head".to_owned(),
            oid: commit_oid(root, "origin/HEAD")?,
        });
    }
    Ok(BaseRef {
        name: "HEAD".to_owned(),
        kind: "head".to_owned(),
        oid: commit_oid(root, "HEAD")?,
    })
}

pub(crate) fn commit_exists(root: &Path, reference: &str) -> Result<bool, WorktreeError> {
    let status = git_status(
        root,
        &[
            os("rev-parse"),
            os("--verify"),
            os("--quiet"),
            os(&format!("{reference}^{{commit}}")),
        ],
    )?;
    match status.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(WorktreeError::GitFailed {
            command: status.command,
            status: status.status.code(),
            stdout: status.stdout,
            stderr: status.stderr,
        }),
    }
}

pub(crate) fn commit_oid(root: &Path, reference: &str) -> Result<String, WorktreeError> {
    let output = git_output(
        root,
        &[
            os("rev-parse"),
            os("--verify"),
            os(&format!("{reference}^{{commit}}")),
        ],
    )?;
    Ok(output.stdout.trim().to_owned())
}

pub(crate) fn is_dirty(path: &Path) -> Result<bool, WorktreeError> {
    let output = git_output(
        path,
        &[
            os("status"),
            os("--porcelain=v1"),
            os("--untracked-files=normal"),
        ],
    )?;
    Ok(!output.stdout.trim().is_empty())
}

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitError {
    #[error("git was not found on PATH")]
    Missing,
    #[error("not a git repository")]
    NotARepository,
    #[error("failed to spawn git: {source}")]
    Spawn { source: std::io::Error },
    #[error("git command failed ({command}) with status {status:?}: {stderr}{stdout}")]
    Failed {
        command: String,
        status: Option<i32>,
        stdout: String,
        stderr: String,
    },
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub struct Output {
    pub stdout: String,
}

pub struct Capture {
    pub command: String,
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Capture {
    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }

    pub fn into_failure(self) -> GitError {
        GitError::Failed {
            command: self.command,
            status: self.status.code(),
            stdout: self.stdout,
            stderr: self.stderr,
        }
    }
}

pub fn capture<S: AsRef<OsStr>>(cwd: &Path, args: &[S]) -> Result<Capture, GitError> {
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
                GitError::Missing
            } else {
                GitError::Spawn { source }
            }
        })?;
    Ok(Capture {
        command,
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub fn output<S: AsRef<OsStr>>(cwd: &Path, args: &[S]) -> Result<Output, GitError> {
    let capture = capture(cwd, args)?;
    if capture.status.success() {
        Ok(Output {
            stdout: capture.stdout,
        })
    } else {
        Err(capture.into_failure())
    }
}

fn format_command<S: AsRef<OsStr>>(args: &[S]) -> String {
    let mut parts = vec!["git".to_owned()];
    parts.extend(
        args.iter()
            .map(|arg| arg.as_ref().to_string_lossy().into_owned()),
    );
    parts.join(" ")
}

#[cfg(windows)]
pub fn path_arg(path: &Path) -> OsString {
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
pub fn path_arg(path: &Path) -> OsString {
    path.as_os_str().to_os_string()
}

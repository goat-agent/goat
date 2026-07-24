use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{error::ToolError, path::resolve_with_policy, policy::SandboxPolicy};

pub struct ToolContext {
    pub cwd: PathBuf,
    pub bash_timeout: Duration,
    pub max_output_bytes: usize,
    pub extra_path: Option<PathBuf>,
    pub blocked_paths: Vec<PathBuf>,
    pub write_allow: Option<PathBuf>,
    pub exec_policy: SandboxPolicy,
}

impl ToolContext {
    pub fn new(cwd: &Path) -> Result<Self, ToolError> {
        let cwd = cwd
            .canonicalize()
            .map_err(|source| ToolError::Cwd { source })?;
        let blocked_paths = workspace_blocked_paths(&cwd);
        Ok(Self {
            cwd,
            bash_timeout: Duration::from_mins(2),
            max_output_bytes: 64 * 1024,
            extra_path: None,
            blocked_paths,
            write_allow: None,
            exec_policy: SandboxPolicy::Full,
        })
    }

    pub fn resolve(&self, raw: &str) -> Result<PathBuf, ToolError> {
        resolve_with_policy(
            &self.cwd,
            self.extra_path.as_deref(),
            &self.blocked_paths,
            raw,
        )
    }

    pub fn ensure_writable(&self, resolved: &Path, raw: &str) -> Result<(), ToolError> {
        match &self.write_allow {
            Some(allowed) if resolved != allowed.as_path() => Err(ToolError::WriteBlocked {
                path: raw.to_owned(),
            }),
            _ => Ok(()),
        }
    }
}

fn workspace_blocked_paths(cwd: &Path) -> Vec<PathBuf> {
    let managed = cwd.join(".goat").join("worktrees");
    if managed.exists() {
        match managed.canonicalize() {
            Ok(path) => vec![path],
            Err(_) => vec![managed],
        }
    } else {
        vec![managed]
    }
}

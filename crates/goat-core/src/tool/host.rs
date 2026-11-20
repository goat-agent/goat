use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{Call, Kind, Step, bash, danger, files, hashline};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const NO_TIMEOUT: Duration = Duration::MAX;

#[derive(Debug)]
pub(crate) struct Host {
    workspace: PathBuf,
    home: PathBuf,
    spill: PathBuf,
    danger: danger::Checker,
    steps: mpsc::Sender<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Op {
    Bash {
        command: String,
        timeout: Option<u64>,
    },
    Read {
        path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    Edit {
        path: String,
        edits: Vec<hashline::RawOp>,
    },
    Write {
        path: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Reply<T> {
    Ok(T),
    Error(String),
}

impl Host {
    pub(crate) fn new(
        workspace: &Path,
        home: &Path,
        spill: &Path,
        steps: mpsc::Sender<Step>,
    ) -> Result<Self, tree_sitter::LanguageError> {
        Ok(Self {
            workspace: workspace.to_owned(),
            home: home.to_owned(),
            spill: spill.to_owned(),
            danger: danger::Checker::new()?,
            steps,
        })
    }

    pub(crate) fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub(crate) fn spill(&self) -> &Path {
        &self.spill
    }

    pub(crate) async fn call(&self, request: &str) -> String {
        let reply = match serde_json::from_str::<Op>(request) {
            Ok(op) => {
                self.steps.send(Step::Started(op.to_call())).await.ok();
                let reply = self.dispatch(op).await;
                if let Err(problem) = &reply {
                    self.steps
                        .send(Step::Failed(problem.as_str().into()))
                        .await
                        .ok();
                }
                reply
            }
            Err(err) => Err(format!("invalid call: {err}")),
        };
        let reply = match reply {
            Ok(value) => Reply::Ok(value),
            Err(problem) => Reply::Error(problem),
        };
        serde_json::to_string(&reply)
            .unwrap_or_else(|err| format!(r#"{{"error":"reply could not be encoded: {err}"}}"#))
    }

    async fn dispatch(&self, op: Op) -> Result<serde_json::Value, String> {
        match op {
            Op::Bash { command, timeout } => to_value(self.bash(&command, timeout).await?),
            Op::Read {
                path,
                offset,
                limit,
            } => to_value(files::read(&self.resolve(&path), &path, offset, limit)?),
            Op::Edit { path, edits } => to_value(files::edit(&self.resolve(&path), &path, edits)?),
            Op::Write { path, content } => {
                to_value(files::write(&self.resolve(&path), &path, &content)?)
            }
        }
    }

    async fn bash(&self, command: &str, timeout: Option<u64>) -> Result<bash::Ran, String> {
        if let Some(reason) = self.danger.check(command, &self.workspace, &self.home) {
            return Err(format!(
                "blocked: {reason}. goat never runs this; the user can run it themselves"
            ));
        }
        let timeout = Duration::from_secs(timeout.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1));
        bash::run(command, &self.workspace, timeout, &self.spill, &mut |_| {})
            .await
            .map_err(|err| format!("could not run the command: {err}"))
    }

    pub(crate) async fn shell(
        &self,
        command: &str,
        on_output: &mut dyn FnMut(&str),
    ) -> Result<bash::Ran, String> {
        if let Some(reason) = self.danger.check(command, &self.workspace, &self.home) {
            return Err(format!("blocked: {reason}"));
        }
        bash::run(command, &self.workspace, NO_TIMEOUT, &self.spill, on_output)
            .await
            .map_err(|err| format!("could not run the command: {err}"))
    }

    fn resolve(&self, raw: &str) -> PathBuf {
        match raw.strip_prefix('~') {
            Some("") => self.home.clone(),
            Some(rest) if rest.starts_with('/') => self.home.join(rest.trim_start_matches('/')),
            _ => self.workspace.join(raw),
        }
    }
}

impl Op {
    fn to_call(&self) -> Call {
        let (kind, target) = match self {
            Self::Bash { command, .. } => (Kind::Bash, command),
            Self::Read { path, .. } => (Kind::Read, path),
            Self::Edit { path, .. } => (Kind::Edit, path),
            Self::Write { path, .. } => (Kind::Write, path),
        };
        Call {
            kind,
            target: target.as_str().into(),
        }
    }
}

fn to_value(value: impl Serialize) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|err| format!("result could not be encoded: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(dir: &Path) -> (Host, mpsc::Receiver<Step>) {
        let (tx, rx) = mpsc::channel(16);
        let host = Host::new(dir, Path::new("/home/me"), &dir.join(".spill"), tx).unwrap();
        (host, rx)
    }

    #[tokio::test]
    async fn sh_returns_exit_code_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let (host, _rx) = host(dir.path());
        let reply = host
            .call(r#"{"op":"bash","command":"echo hi; exit 2","timeout":null}"#)
            .await;
        assert_eq!(reply, r#"{"ok":{"code":2,"file":null,"output":"hi\n"}}"#);
    }

    #[tokio::test]
    async fn dangerous_command_is_blocked_before_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (host, mut rx) = host(dir.path());
        let reply = host.call(r#"{"op":"bash","command":"rm -rf ~"}"#).await;
        assert!(reply.starts_with(r#"{"error":"blocked:"#));
        assert!(
            matches!(rx.recv().await, Some(Step::Started(call)) if call.kind == Kind::Bash && &*call.target == "rm -rf ~")
        );
        assert!(matches!(rx.recv().await, Some(Step::Failed(_))));
    }

    #[tokio::test]
    async fn relative_paths_resolve_in_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let (host, _rx) = host(dir.path());
        host.call(r#"{"op":"write","path":"a/b.txt","content":"x\n"}"#)
            .await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b.txt")).unwrap(),
            "x\n"
        );
    }

    #[tokio::test]
    async fn unknown_operation_is_an_error_reply() {
        let dir = tempfile::tempdir().unwrap();
        let (host, _rx) = host(dir.path());
        let reply = host.call(r#"{"op":"browse"}"#).await;
        assert!(reply.starts_with(r#"{"error":"invalid call:"#));
    }
}

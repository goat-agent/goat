use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::web::{self, Web};
use super::{Call, Kind, Step, bash, danger, files, hashline};
use crate::image;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const NO_TIMEOUT: Duration = Duration::MAX;

#[derive(Debug)]
pub(crate) struct Host {
    workspace: PathBuf,
    home: PathBuf,
    spill: PathBuf,
    danger: danger::Checker,
    steps: mpsc::Sender<Step>,
    web: RefCell<Option<Web>>,
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
    Fetch {
        url: String,
    },
    Search {
        query: String,
    },
}

#[derive(Debug, Serialize)]
struct Shown {
    image: String,
    width: u32,
    height: u32,
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
            web: RefCell::new(None),
        })
    }

    pub(crate) fn set_web(&self, web: Web) {
        self.web.replace(Some(web));
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
            } => {
                let resolved = self.resolve(&path);
                match files::image(&resolved, &path)? {
                    Some(bytes) => to_value(self.show(bytes, &path).await?),
                    None => to_value(files::read(&resolved, &path, offset, limit)?),
                }
            }
            Op::Edit { path, edits } => {
                let (edited, diff) = files::edit(&self.resolve(&path), &path, edits)?;
                self.steps.send(Step::Changed(diff)).await.ok();
                to_value(edited)
            }
            Op::Write { path, content } => {
                let (written, diff) = files::write(&self.resolve(&path), &path, &content)?;
                if let Some(diff) = diff {
                    self.steps.send(Step::Changed(diff)).await.ok();
                }
                to_value(written)
            }
            Op::Fetch { url } => {
                let web = self.web()?;
                to_value(web::fetch(&web.http, &url, &self.spill).await?)
            }
            Op::Search { query } => {
                let searcher = self
                    .web()?
                    .search
                    .ok_or("web search needs a signed-in account")?;
                to_value(web::search(&searcher, &query).await?)
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

    async fn show(&self, bytes: Vec<u8>, path: &str) -> Result<Shown, String> {
        let picture =
            image::prepare(bytes).map_err(|err| format!("could not read {path}: {err}"))?;
        let shown = Shown {
            image: path.to_owned(),
            width: picture.width,
            height: picture.height,
        };
        self.steps.send(Step::Shown(picture)).await.ok();
        Ok(shown)
    }

    fn web(&self) -> Result<Web, String> {
        self.web
            .borrow()
            .clone()
            .ok_or_else(|| "the web is not available here".to_owned())
    }

    pub(crate) fn resolve(&self, raw: &str) -> PathBuf {
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
            Self::Fetch { url } => (Kind::Fetch, url),
            Self::Search { query } => (Kind::Search, query),
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
    async fn reading_an_image_shows_it_and_returns_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let (host, mut rx) = host(dir.path());
        ::image::RgbImage::new(6, 4)
            .save(dir.path().join("shot.png"))
            .unwrap();

        let reply = host
            .call(r#"{"op":"read","path":"shot.png","offset":null,"limit":null}"#)
            .await;

        assert_eq!(reply, r#"{"ok":{"height":4,"image":"shot.png","width":6}}"#);
        assert!(matches!(rx.recv().await, Some(Step::Started(_))));
        assert!(matches!(rx.recv().await, Some(Step::Shown(picture)) if picture.width == 6));
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

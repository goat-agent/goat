use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::lsp::{self, Servers, Target};
use super::web::{self, Web};
use super::{
    Call, Decision, Kind, Question, Scope, Spawn, Step, Task, bash, danger, files, hashline, spill,
};
use crate::mcp::Clients;
use crate::skill::{self, Skill};
use crate::{image, instructions};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const NO_TIMEOUT: Duration = Duration::MAX;

#[derive(Debug)]
pub(crate) struct Host {
    workspace: PathBuf,
    home: PathBuf,
    spill: PathBuf,
    danger: danger::Checker,
    steps: mpsc::UnboundedSender<Step>,
    asking: tokio::sync::Semaphore,
    allowed: RefCell<HashSet<String>>,
    web: RefCell<Option<Web>>,
    nested: RefCell<Nested>,
    skills: RefCell<Skills>,
    mcp: RefCell<Option<Rc<Clients>>>,
    spawner: RefCell<Option<Rc<dyn Spawn>>>,
    lsp: RefCell<Option<Rc<Servers>>>,
}

#[derive(Debug, Default)]
struct Skills {
    known: Vec<Skill>,
    active: HashSet<String>,
    pending: Vec<String>,
}

#[derive(Debug, Default)]
struct Nested {
    loaded: HashSet<PathBuf>,
    pending: Vec<PathBuf>,
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
    Skill {
        name: String,
        args: Option<String>,
    },
    McpTools {
        server: Option<String>,
    },
    McpDescribe {
        name: String,
    },
    McpCall {
        server: String,
        tool: String,
        args: serde_json::Value,
    },
    Agent {
        task: String,
        #[serde(rename = "as")]
        agent: Option<String>,
        id: Option<String>,
    },
    Lsp {
        action: Ask,
        path: Option<String>,
        line: Option<String>,
        name: Option<String>,
        new_name: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ask {
    Definition,
    References,
    Implementations,
    Hover,
    Symbols,
    Rename,
    Diagnostics,
}

#[derive(Debug, Serialize)]
struct Renamed {
    files: Vec<String>,
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
        steps: mpsc::UnboundedSender<Step>,
    ) -> Result<Self, tree_sitter::LanguageError> {
        Ok(Self {
            workspace: workspace.to_owned(),
            home: home.to_owned(),
            spill: spill.to_owned(),
            danger: danger::Checker::new()?,
            steps,
            asking: tokio::sync::Semaphore::new(1),
            allowed: RefCell::new(HashSet::new()),
            web: RefCell::new(None),
            nested: RefCell::new(Nested::default()),
            skills: RefCell::new(Skills::default()),
            mcp: RefCell::new(None),
            spawner: RefCell::new(None),
            lsp: RefCell::new(None),
        })
    }

    pub(crate) fn start_run(&self, web: Web) {
        self.web.replace(Some(web));
        self.allowed.borrow_mut().clear();
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
                self.steps.send(Step::Started(op.to_call())).ok();
                let reply = self.dispatch(op).await;
                if let Err(problem) = &reply {
                    self.steps.send(Step::Failed(problem.as_str().into())).ok();
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
                self.guard(&resolved)?;
                let value = match files::image(&resolved, &path)? {
                    Some(bytes) => to_value(self.show(bytes, &path)?),
                    None => to_value(files::read(&resolved, &path, offset, limit)?),
                };
                self.notice(&resolved);
                if let Some(servers) = self.servers() {
                    servers.saw(&resolved).await;
                }
                value
            }
            Op::Edit { path, edits } => {
                let resolved = self.resolve(&path);
                self.guard(&resolved)?;
                let (edited, diff) = files::edit(&resolved, &path, edits)?;
                self.notice(&resolved);
                self.touched(&resolved);
                self.steps.send(Step::Changed(diff)).ok();
                to_value(edited)
            }
            Op::Write { path, content } => {
                let resolved = self.resolve(&path);
                self.guard(&resolved)?;
                let (written, diff) = files::write(&resolved, &path, &content)?;
                self.notice(&resolved);
                self.touched(&resolved);
                if let Some(diff) = diff {
                    self.steps.send(Step::Changed(diff)).ok();
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
            Op::McpTools { server } => to_value(self.mcp()?.tools(server.as_deref()).await?),
            Op::McpDescribe { name } => to_value(self.mcp()?.describe(&name).await?),
            Op::McpCall { server, tool, args } => {
                let clients = self.mcp()?;
                if clients.is_destructive(&server, &tool).await {
                    self.confirm(&format!("{server}.{tool}"), &args).await?;
                }
                let called = clients.call(&server, &tool, args).await?;
                for bytes in called.images {
                    self.show(bytes, &format!("{server}.{tool}"))?;
                }
                match called.value {
                    serde_json::Value::String(text) => to_value(spill::fit(text, &self.spill)),
                    value => Ok(value),
                }
            }
            Op::Skill { name, args } => {
                to_value(self.load_skill(&name, args.as_deref().unwrap_or_default())?)
            }
            Op::Agent { task, agent, id } => {
                let spawner = self
                    .spawner
                    .borrow()
                    .clone()
                    .ok_or("agents cannot start agents")?;
                let task = Task {
                    text: task,
                    agent,
                    id,
                };
                to_value(spawner.spawn(task, &self.steps).await?)
            }
            Op::Lsp {
                action,
                path,
                line,
                name,
                new_name,
            } => self.ask(action, path, line, name, new_name).await,
        }
    }

    async fn ask(
        &self,
        action: Ask,
        path: Option<String>,
        line: Option<String>,
        name: Option<String>,
        new_name: Option<String>,
    ) -> Result<serde_json::Value, String> {
        let servers = self
            .servers()
            .ok_or("language servers are not available here")?;
        let resolved = match &path {
            Some(path) => {
                let resolved = self.resolve(path);
                self.guard(&resolved)?;
                Some(resolved)
            }
            None => None,
        };
        let spot = || -> Result<(PathBuf, String, String), String> {
            let missing = |what: &str| format!("{} needs {what}", action.as_str());
            Ok((
                resolved.clone().ok_or_else(|| missing("a path"))?,
                line.clone().ok_or_else(|| missing("a line"))?,
                name.clone().ok_or_else(|| missing("a name"))?,
            ))
        };
        let located = match action {
            Ask::Diagnostics => return to_value(servers.diagnostics(resolved.as_deref()).await?),
            Ask::Symbols => {
                return match (&resolved, path) {
                    (Some(resolved), _) if resolved.is_file() => {
                        to_value(servers.outline(resolved).await?)
                    }
                    (_, Some(query)) => to_value(servers.search(&query).await?),
                    (_, None) => Err("symbols needs a path or a query".to_owned()),
                };
            }
            Ask::Rename => {
                let (path, line, name) = spot()?;
                let new_name = new_name.ok_or("rename needs a new name")?;
                let target = Target {
                    path: &path,
                    line: &line,
                    name: &name,
                };
                return to_value(self.rename(&servers, &target, &new_name).await?);
            }
            Ask::Definition => lsp::Action::Definition,
            Ask::References => lsp::Action::References,
            Ask::Implementations => lsp::Action::Implementations,
            Ask::Hover => lsp::Action::Hover,
        };
        let (path, line, name) = spot()?;
        let target = Target {
            path: &path,
            line: &line,
            name: &name,
        };
        servers.locate(&target, located).await
    }

    async fn rename(
        &self,
        servers: &Servers,
        target: &Target<'_>,
        new_name: &str,
    ) -> Result<Renamed, String> {
        let changes = servers.rename(target, new_name).await?;
        for (path, _) in &changes {
            self.guard(path)?;
        }
        let mut renamed = Vec::with_capacity(changes.len());
        for (path, content) in changes {
            let shown = path
                .strip_prefix(&self.workspace)
                .unwrap_or(&path)
                .display()
                .to_string();
            let (_, diff) = files::write(&path, &shown, &content)?;
            self.touched(&path);
            if let Some(diff) = diff {
                self.steps.send(Step::Changed(diff)).ok();
            }
            renamed.push(shown);
        }
        Ok(Renamed { files: renamed })
    }

    pub(crate) async fn check(&self) -> Option<String> {
        self.servers()?.report().await
    }

    fn touched(&self, path: &Path) {
        if let Some(servers) = self.servers() {
            servers.changed(path);
        }
    }

    fn servers(&self) -> Option<Rc<Servers>> {
        self.lsp.borrow().clone()
    }

    pub(crate) fn set_lsp(&self, servers: Rc<Servers>) {
        self.lsp.replace(Some(servers));
    }

    pub(crate) fn steps(&self) -> mpsc::UnboundedSender<Step> {
        self.steps.clone()
    }

    async fn confirm(&self, action: &str, args: &serde_json::Value) -> Result<(), String> {
        let _turn = self.asking.acquire().await.map_err(|err| err.to_string())?;
        if self.allowed.borrow().contains(action) {
            return Ok(());
        }
        let call = format!("{action}({args})");
        let (question, answer) = Question::new(call.clone(), Scope::Run);
        self.steps.send(Step::Confirm(question)).ok();
        match answer.await {
            Ok(Decision::Once | Decision::Always) => Ok(()),
            Ok(Decision::Run) => {
                self.allowed.borrow_mut().insert(action.to_owned());
                Ok(())
            }
            Ok(Decision::Refuse) => Err(format!("the user refused {call}")),
            Err(_) => Err(format!("{call} did not run: the question was not answered")),
        }
    }

    async fn bash(&self, command: &str, timeout: Option<u64>) -> Result<bash::Ran, String> {
        if let Some(reason) = self.danger.check(command, &self.workspace, &self.home) {
            return Err(format!(
                "blocked: {reason}. goat never runs this; the user can run it themselves"
            ));
        }
        if let Some(servers) = self.servers() {
            servers.shelling();
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
        if let Some(servers) = self.servers() {
            servers.shelling();
        }
        bash::run(command, &self.workspace, NO_TIMEOUT, &self.spill, on_output)
            .await
            .map_err(|err| format!("could not run the command: {err}"))
    }

    fn show(&self, bytes: Vec<u8>, path: &str) -> Result<Shown, String> {
        let picture =
            image::prepare(bytes).map_err(|err| format!("could not read {path}: {err}"))?;
        let shown = Shown {
            image: path.to_owned(),
            width: picture.width,
            height: picture.height,
        };
        self.steps.send(Step::Shown(picture)).ok();
        Ok(shown)
    }

    fn web(&self) -> Result<Web, String> {
        self.web
            .borrow()
            .clone()
            .ok_or_else(|| "the web is not available here".to_owned())
    }

    pub(crate) fn set_spawner(&self, spawner: Option<Rc<dyn Spawn>>) {
        self.spawner.replace(spawner);
    }

    pub(crate) fn set_mcp(&self, clients: Rc<Clients>) {
        self.mcp.replace(Some(clients));
    }

    fn mcp(&self) -> Result<Rc<Clients>, String> {
        self.mcp
            .borrow()
            .clone()
            .ok_or_else(|| "no MCP servers are configured".to_owned())
    }

    pub(crate) fn set_skills(&self, known: Vec<Skill>) {
        self.skills.borrow_mut().known = known;
    }

    pub(crate) fn mark_skill(&self, name: &str) -> bool {
        self.skills.borrow_mut().active.insert(name.to_owned())
    }

    pub(crate) fn active_skills(&self) -> Vec<String> {
        let mut active: Vec<String> = self.skills.borrow().active.iter().cloned().collect();
        active.sort();
        active
    }

    pub(crate) fn take_skills(&self) -> Vec<String> {
        std::mem::take(&mut self.skills.borrow_mut().pending)
    }

    fn load_skill(&self, name: &str, args: &str) -> Result<String, String> {
        let mut skills = self.skills.borrow_mut();
        let skill = skills
            .known
            .iter()
            .find(|skill| skill.name == name && skill.for_model)
            .ok_or_else(|| format!("there is no skill named {name}"))?;
        let text = if skills.active.contains(name) {
            let mut text = format!("The {name} skill is already loaded above; follow it.");
            if !args.is_empty() {
                text.push_str("\n\nARGUMENTS: ");
                text.push_str(args);
            }
            text
        } else {
            skill::activate(skill, args)?
        };
        skills.active.insert(name.to_owned());
        skills.pending.push(text);
        Ok(format!(
            "The {name} skill's instructions come after this run's result."
        ))
    }

    pub(crate) fn mark_loaded(&self, path: PathBuf) {
        self.nested.borrow_mut().loaded.insert(path);
    }

    pub(crate) fn loaded(&self) -> Vec<PathBuf> {
        let mut loaded: Vec<PathBuf> = self.nested.borrow().loaded.iter().cloned().collect();
        loaded.sort();
        loaded
    }

    pub(crate) fn take_instructions(&self) -> Vec<(String, String)> {
        let pending = std::mem::take(&mut self.nested.borrow_mut().pending);
        pending
            .into_iter()
            .filter_map(|path| {
                let text = instructions::read(&path, instructions::BYTES);
                let shown = instructions::shown(&path, &self.workspace, &self.home);
                self.mark_loaded(path);
                Some((shown, text?))
            })
            .collect()
    }

    fn notice(&self, path: &Path) {
        let Some(relative) = path
            .strip_prefix(&self.workspace)
            .ok()
            .and_then(Path::parent)
        else {
            return;
        };
        let mut dir = self.workspace.clone();
        let mut nested = self.nested.borrow_mut();
        for component in relative.components() {
            let Component::Normal(part) = component else {
                return;
            };
            dir.push(part);
            let file = dir.join(instructions::FILE);
            if !nested.loaded.contains(&file) && !nested.pending.contains(&file) && file.is_file() {
                nested.pending.push(file);
            }
        }
    }

    fn guard(&self, resolved: &Path) -> Result<(), String> {
        if real(resolved).starts_with(real(&self.home.join(".goat"))) {
            return Err(
                "goat keeps its sign-ins in ~/.goat, so runs may not read or change it".to_owned(),
            );
        }
        Ok(())
    }

    pub(crate) fn resolve(&self, raw: &str) -> PathBuf {
        match raw.strip_prefix('~') {
            Some("") => self.home.clone(),
            Some(rest) if rest.starts_with('/') => self.home.join(rest.trim_start_matches('/')),
            _ => self.workspace.join(raw),
        }
    }
}

fn real(path: &Path) -> PathBuf {
    let mut plain = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                plain.pop();
            }
            Component::CurDir => {}
            other => plain.push(other),
        }
    }
    let mut rest = Vec::new();
    let mut base = plain.as_path();
    loop {
        if let Ok(found) = base.canonicalize() {
            return rest.iter().rev().fold(found, |path, part| path.join(part));
        }
        let (Some(parent), Some(name)) = (base.parent(), base.file_name()) else {
            return plain;
        };
        rest.push(name.to_owned());
        base = parent;
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
            Self::Skill { name, .. } => (Kind::Skill, name),
            Self::McpTools { server } => {
                return Call {
                    kind: Kind::Mcp,
                    target: server
                        .as_deref()
                        .map_or("tools".into(), |server| format!("{server} tools").into()),
                };
            }
            Self::McpDescribe { name } => (Kind::Mcp, name),
            Self::McpCall { server, tool, .. } => {
                return Call {
                    kind: Kind::Mcp,
                    target: format!("{server}.{tool}").into(),
                };
            }
            Self::Lsp {
                action, path, line, ..
            } => {
                let mut target = action.as_str().to_owned();
                for part in [path, line].into_iter().flatten() {
                    target.push(' ');
                    target.push_str(part);
                }
                return Call {
                    kind: Kind::Lsp,
                    target: target.into(),
                };
            }
            Self::Agent { task, agent, .. } => {
                return Call {
                    kind: Kind::Agent,
                    target: match agent {
                        Some(agent) => format!("{agent}: {task}").into(),
                        None => task.as_str().into(),
                    },
                };
            }
        };
        Call {
            kind,
            target: target.as_str().into(),
        }
    }
}

impl Ask {
    fn as_str(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::References => "references",
            Self::Implementations => "implementations",
            Self::Hover => "hover",
            Self::Symbols => "symbols",
            Self::Rename => "rename",
            Self::Diagnostics => "diagnostics",
        }
    }
}

fn to_value(value: impl Serialize) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|err| format!("result could not be encoded: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(dir: &Path) -> (Host, mpsc::UnboundedReceiver<Step>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let host = Host::new(dir, Path::new("/home/me"), &dir.join(".spill"), tx).unwrap();
        (host, rx)
    }

    #[tokio::test]
    async fn questions_come_one_at_a_time_and_a_run_answer_covers_that_tool() {
        let dir = tempfile::tempdir().unwrap();
        let (host, mut rx) = host(dir.path());
        let args = serde_json::json!({ "threadId": "t1" });
        let asking = async {
            tokio::join!(
                host.confirm("gmail.trash_thread", &args),
                host.confirm("gmail.trash_thread", &args),
                host.confirm("gmail.delete_label", &args),
            )
        };
        let answering = async {
            let Some(Step::Confirm(first)) = rx.recv().await else {
                panic!("the first call asks");
            };
            let is_alone = rx.try_recv().is_err();
            first.take_answer().unwrap().give(Decision::Run);
            let Some(Step::Confirm(second)) = rx.recv().await else {
                panic!("another tool asks again");
            };
            second.take_answer().unwrap().give(Decision::Refuse);
            (first.text().to_owned(), is_alone, second.text().to_owned())
        };

        let (answers, (first, is_alone, second)) = tokio::join!(asking, answering);

        assert_eq!(first, r#"gmail.trash_thread({"threadId":"t1"})"#);
        assert!(is_alone);
        assert_eq!(second, r#"gmail.delete_label({"threadId":"t1"})"#);
        assert_eq!(
            answers,
            (
                Ok(()),
                Ok(()),
                Err(r#"the user refused gmail.delete_label({"threadId":"t1"})"#.to_owned())
            )
        );
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn an_unanswered_question_is_not_taken_for_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let (host, mut rx) = host(dir.path());
        let args = serde_json::json!({ "threadId": "t1" });

        let (answer, ()) = tokio::join!(host.confirm("gmail.trash_thread", &args), async {
            drop(rx.recv().await);
        });

        assert_eq!(
            answer,
            Err(
                r#"gmail.trash_thread({"threadId":"t1"}) did not run: the question was not answered"#
                    .to_owned()
            )
        );
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
    async fn goats_own_files_cannot_be_read_or_written() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".goat")).unwrap();
        std::fs::write(home.join(".goat/mcp.toml"), "[notes]\n").unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let host = Host::new(dir.path(), &home, &dir.path().join(".spill"), tx).unwrap();

        let read = host
            .call(r#"{"op":"read","path":"~/.goat/mcp.toml","offset":null,"limit":null}"#)
            .await;
        let written = host
            .call(r#"{"op":"write","path":"home/.goat/x","content":"y"}"#)
            .await;

        assert!(
            read.contains("goat keeps its sign-ins in ~/.goat"),
            "{read}"
        );
        assert!(
            written.contains("goat keeps its sign-ins in ~/.goat"),
            "{written}"
        );
        assert!(!home.join(".goat/x").exists());
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

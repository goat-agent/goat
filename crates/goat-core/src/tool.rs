mod bash;
mod danger;
mod diff;
mod files;
mod hashline;
mod host;
mod mention;
mod script;
mod spill;
mod web;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use crate::image::Picture;
use crate::instructions;
use crate::message::Diff;
use crate::skill::Skill;

pub(crate) use self::web::{Searcher, Web};

use self::host::Host;
pub use self::mention::workspace_files;
pub(crate) use self::mention::{Attached, mentioned};
use self::script::Script;

const NAME: &str = "run";
const IMAGES_PER_RUN: usize = 4;
const STEP_BACKLOG: usize = 64;
const NEW_SANDBOX: &str =
    "[this run started a new sandbox; values earlier runs kept on globalThis are gone]";

const SCHEMA: &str = r#"{"type":"object","properties":{"code":{"type":"string","description":"JavaScript to run; top-level await works, and the result is the last expression or what a top-level return gives"},"description":{"type":"string","description":"What this run does in 3 to 8 words, shown to the user"}},"required":["code","description"],"additionalProperties":false}"#;
const DESCRIPTION: &str = "\
Run JavaScript in a sandbox to work in the workspace. Top-level await works, and the value of the last expression, or what a top-level return gives, comes back to you. \
let, const and class declarations belong to their run, so a later run can declare the same names. To keep a value for later runs, put it on globalThis, as in globalThis.files = await bash`ls`. \
Do steps that belong together in one run, such as search, read, edit and test, and return only what you need to see; nothing else enters the conversation.

The sandbox reaches the system only through these async functions:
- bash`cmd ${value}` or bash(command, {timeout}) runs bash -c in the working directory. Template values are shell-quoted. Returns {code, output, file}: code is null when the command was killed or timed out (default 120 seconds), output holds stdout and stderr cut to their first and last 20 KB, and file names the full output when it was cut. A non-zero exit is returned, not thrown.
- read(path, {offset, limit}) returns {lines, total, next}. Each line is `N:hhh|text`, where `N:hhh` is its anchor. It gives up to 2000 lines from offset (1-based); next is the offset to continue from, or null. On a PNG, JPEG, GIF or WebP file it returns {image, width, height} instead, and you see the image after the run's result; a run shows at most 4 images.
- edit(path, edits) changes lines of an existing file. Each edit is {op: \"replace\" | \"insert_after\" | \"delete\", from, to, lines}. from and to are lines or anchors from your latest read or edit of that file; to defaults to from; lines are the new lines; insert_after without from inserts at the top. Edits apply together or not at all, and a line that changed since it was read rejects them. Returns {total, preview}, where preview shows the changed lines with their new anchors.
- write(path, content) creates a file or replaces a whole file. Returns {created, lines}.
- webFetch(url) gets a web page like the standard fetch and returns {ok, status, url, text(), json()}, where text() of an HTML page is markdown. The text is cut to its first and last 20 KB with the full text in a file. Only http and https URLs without credentials are fetched; use bash and curl for anything else.
- webSearch(query) searches the web and returns up to 8 results as [{title, url, snippet}].
- skill(name, args) loads a skill listed in your instructions; its instructions come after the run's result. args is the user's text for it, or omitted.
- console.log(...) adds lines to what you see, before the result.

A failed read, edit or write, and a blocked command, throw an Error. A run returns only after all its calls finish, so await the calls whose results you use. Paths are relative to the working directory, and ~ is the home directory.";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("load the bash grammar")]
    Grammar(#[from] tree_sitter::LanguageError),
}

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Box<RawValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub output: Box<str>,
    pub is_error: bool,
    pub images: Vec<Picture>,
}

#[derive(Debug)]
pub(crate) struct Shelled {
    pub(crate) code: Option<i32>,
    pub(crate) output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Bash,
    Read,
    Edit,
    Write,
    Fetch,
    Search,
    Skill,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub kind: Kind,
    pub target: Box<str>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Started(Call),
    Failed(Box<str>),
    Changed(Diff),
    Loaded(Box<str>),
    Shown(Picture),
    Confirm(Question),
}

#[derive(Debug)]
pub struct Question {
    text: Box<str>,
    answer: std::cell::RefCell<Option<Answer>>,
}

#[derive(Debug)]
pub struct Answer(tokio::sync::oneshot::Sender<bool>);

impl Question {
    pub(crate) fn new(text: String) -> (Self, tokio::sync::oneshot::Receiver<bool>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let question = Self {
            text: text.into(),
            answer: std::cell::RefCell::new(Some(Answer(tx))),
        };
        (question, rx)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn take_answer(&self) -> Option<Answer> {
        self.answer.borrow_mut().take()
    }
}

impl PartialEq for Question {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for Question {}

impl Answer {
    pub fn give(self, is_allowed: bool) {
        self.0.send(is_allowed).ok();
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Run {
    code: String,
    description: String,
}

pub(crate) struct Runner {
    host: Rc<Host>,
    steps: mpsc::Receiver<Step>,
    script: Option<Script>,
    had_sandbox: bool,
}

impl Outcome {
    pub(crate) fn ok(output: impl Into<Box<str>>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            images: Vec::new(),
        }
    }

    pub(crate) fn error(output: impl Into<Box<str>>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            images: Vec::new(),
        }
    }
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Write => "write",
            Self::Fetch => "fetch",
            Self::Search => "search",
            Self::Skill => "skill",
            Self::Mcp => "mcp",
        }
    }
}

impl Run {
    pub(crate) fn parse(name: &str, input: &str) -> Result<Self, String> {
        if name != NAME {
            return Err(format!("there is no tool named {name}; use {NAME}"));
        }
        serde_json::from_str(input).map_err(|err| format!("invalid arguments for {NAME}: {err}"))
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }
}

impl Runner {
    pub(crate) fn new(
        workspace: &Path,
        home: &Path,
        spill: &Path,
        resumed: bool,
    ) -> Result<Self, Error> {
        let (tx, steps) = mpsc::channel(STEP_BACKLOG);
        Ok(Self {
            host: Rc::new(Host::new(workspace, home, spill, tx)?),
            steps,
            script: None,
            had_sandbox: resumed,
        })
    }

    pub(crate) fn workspace(&self) -> &Path {
        self.host.workspace()
    }

    pub(crate) fn mark_loaded(&self, path: PathBuf) {
        self.host.mark_loaded(path);
    }

    pub(crate) fn set_skills(&self, skills: Vec<Skill>) {
        self.host.set_skills(skills);
    }

    pub(crate) fn set_mcp(&self, clients: Rc<crate::mcp::Clients>) {
        self.host.set_mcp(clients);
    }

    pub(crate) fn mark_skill(&self, name: &str) -> bool {
        self.host.mark_skill(name)
    }

    pub(crate) fn active_skills(&self) -> Vec<String> {
        self.host.active_skills()
    }

    pub(crate) fn loaded(&self) -> Vec<PathBuf> {
        self.host.loaded()
    }

    pub(crate) fn attach(&self, raw: &str) -> Option<Attached> {
        mention::attach(|raw| self.host.resolve(raw), self.host.spill(), raw)
    }

    pub(crate) async fn shell(
        &self,
        command: &str,
        on_output: &mut dyn FnMut(&str),
    ) -> Result<Shelled, String> {
        let ran = self.host.shell(command, on_output).await?;
        Ok(Shelled {
            code: ran.code,
            output: ran.output,
        })
    }

    pub(crate) async fn run(
        &mut self,
        run: &Run,
        web: Web,
        on_step: &mut dyn FnMut(Step),
    ) -> Outcome {
        while self.steps.try_recv().is_ok() {}
        self.host.set_web(web);
        let (script, note) = match self.script.take() {
            Some(script) => (script, None),
            None => match Script::new(Rc::clone(&self.host)).await {
                Ok(script) => (script, self.had_sandbox.then_some(NEW_SANDBOX)),
                Err(err) => return Outcome::error(format!("could not start the sandbox: {err}")),
            },
        };
        self.had_sandbox = true;
        let mut images = Vec::new();
        let mut take = |step| match step {
            Step::Shown(picture) => images.push(picture),
            step => on_step(step),
        };
        let outcome = {
            let eval = script.eval(&run.code);
            tokio::pin!(eval);
            loop {
                tokio::select! {
                    outcome = &mut eval => break outcome,
                    Some(step) = self.steps.recv() => take(step),
                }
            }
        };
        while let Ok(step) = self.steps.try_recv() {
            take(step);
        }
        let skills = self.host.take_skills();
        let loaded = self.host.take_instructions();
        for (shown, _) in &loaded {
            on_step(Step::Loaded(shown.as_str().into()));
        }
        self.script = Some(script);
        let dropped = images.len().saturating_sub(IMAGES_PER_RUN);
        images.truncate(IMAGES_PER_RUN);
        let mut text = match note {
            Some(note) => format!("{note}\n{}", outcome.output),
            None => outcome.output.into(),
        };
        if dropped > 0 {
            write!(
                text,
                "\n[{dropped} more images were read; a run shows at most {IMAGES_PER_RUN}]"
            )
            .ok();
        }
        let mut output = spill::fit(text, self.host.spill());
        for (shown, found) in &loaded {
            output.push_str("\n\n");
            output.push_str(&instructions::block(shown, found));
        }
        for text in &skills {
            output.push_str("\n\n");
            output.push_str(text);
        }
        Outcome {
            output: output.into(),
            is_error: outcome.is_error,
            images,
        }
    }
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("workspace", &self.host.workspace())
            .field("has_sandbox", &self.script.is_some())
            .finish_non_exhaustive()
    }
}

pub fn spec() -> Spec {
    Spec {
        name: NAME,
        description: DESCRIPTION,
        schema: RawValue::from_string(SCHEMA.to_owned())
            .unwrap_or_else(|err| unreachable!("the run schema is valid json: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn offline() -> Web {
        Web {
            http: reqwest::Client::new(),
            search: None,
        }
    }

    fn runner(dir: &Path) -> Runner {
        Runner::new(dir, dir, &dir.join(".spill"), false).unwrap()
    }

    fn run(code: &str) -> Run {
        Run {
            code: code.to_owned(),
            description: "test".to_owned(),
        }
    }

    #[test]
    fn spec_schema_is_valid_json() {
        serde_json::from_str::<serde_json::Value>(spec().schema.get()).unwrap();
    }

    #[test]
    fn unknown_tool_is_rejected() {
        assert_eq!(
            Run::parse("bash", "{}").unwrap_err(),
            "there is no tool named bash; use run"
        );
    }

    #[tokio::test]
    async fn fetch_works_like_the_standard_one_and_returns_markdown() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(stream.read(&mut [0; 1024]).await.unwrap() > 0);
            let body = "<h2>Hi</h2>";
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let code = format!("const page = await webFetch('{url}'); [page.ok, await page.text()]");

        let outcome = runner(dir.path())
            .run(&run(&code), offline(), &mut |_| {})
            .await;

        assert_eq!(&*outcome.output, "[\n  true,\n  \"## Hi\"\n]");
    }

    #[tokio::test]
    async fn search_without_an_account_says_so() {
        let dir = tempfile::tempdir().unwrap();

        let outcome = runner(dir.path())
            .run(&run("await webSearch('x')"), offline(), &mut |_| {})
            .await;

        assert!(outcome.is_error);
        assert!(
            outcome
                .output
                .contains("web search needs a signed-in account")
        );
    }

    #[tokio::test]
    async fn steps_are_reported_while_the_script_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut steps = Vec::new();
        runner(dir.path())
            .run(
                &run("await bash`true`; await write('a.txt', 'x')"),
                offline(),
                &mut |step| {
                    steps.push(step);
                },
            )
            .await;
        assert_eq!(
            steps,
            [
                Step::Started(Call {
                    kind: Kind::Bash,
                    target: "true".into(),
                }),
                Step::Started(Call {
                    kind: Kind::Write,
                    target: "a.txt".into(),
                }),
                Step::Changed(Diff {
                    added: 1,
                    removed: 0,
                    lines: vec![crate::message::DiffLine {
                        sign: crate::message::Sign::Added,
                        number: 1,
                        text: "x".into(),
                    }],
                    hidden: 0,
                }),
            ]
        );
    }

    #[tokio::test]
    async fn sandbox_lost_to_a_cancel_is_announced_in_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut runner = runner(dir.path());
        runner
            .run(&run("globalThis.kept = 1"), offline(), &mut |_| {})
            .await;
        let cancelled = tokio::time::timeout(
            Duration::from_millis(300),
            runner.run(&run("await bash`sleep 5`"), offline(), &mut |_| {}),
        )
        .await;
        assert!(cancelled.is_err());

        let outcome = runner
            .run(&run("typeof kept"), offline(), &mut |_| {})
            .await;
        assert_eq!(outcome, Outcome::ok(format!("{NEW_SANDBOX}\nundefined")));
    }

    #[tokio::test]
    async fn cancel_kills_the_commands_of_the_script() {
        let dir = tempfile::tempdir().unwrap();
        let mut runner = runner(dir.path());
        let cancelled = tokio::time::timeout(
            Duration::from_millis(500),
            runner.run(
                &run("await bash`sleep 1; touch late`"),
                offline(),
                &mut |_| {},
            ),
        )
        .await;
        assert!(cancelled.is_err());
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!dir.path().join("late").exists());
    }

    #[tokio::test]
    async fn long_result_is_cut_and_spilled() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = runner(dir.path())
            .run(&run("'x'.repeat(100000)"), offline(), &mut |_| {})
            .await;
        assert!(outcome.output.contains("bytes omitted"));
        assert_eq!(
            std::fs::read_dir(dir.path().join(".spill"))
                .unwrap()
                .count(),
            1
        );
    }
}

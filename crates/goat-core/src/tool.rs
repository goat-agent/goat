mod bash;
mod danger;
mod files;
mod hashline;
mod host;
mod script;
mod spill;

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use self::host::Host;
use self::script::Script;

const NAME: &str = "run";
const SCRIPT_LIMIT: Duration = Duration::from_mins(10);
const STEP_BACKLOG: usize = 64;
const NEW_SANDBOX: &str = "[this run started a new sandbox; variables from earlier runs are gone]";

const SCHEMA: &str = r#"{"type":"object","properties":{"code":{"type":"string","description":"JavaScript to run; top-level await works and the last expression is the result"},"description":{"type":"string","description":"What this run does in 3 to 8 words, shown to the user"}},"required":["code","description"],"additionalProperties":false}"#;
const DESCRIPTION: &str = "\
Run JavaScript in a sandbox to work in the workspace. Top-level await works, the value of the last expression comes back to you, and variables persist between runs. \
Do steps that belong together in one run, such as search, read, edit and test, and return only what you need to see; everything else stays in the sandbox.

The sandbox reaches the system only through these async functions:
- bash`cmd ${value}` or bash(command, {timeout}) runs bash -c in the working directory. Template values are shell-quoted. Returns {code, output, file}: code is null when the command was killed or timed out (default 120 seconds), output holds stdout and stderr cut to their first and last 20 KB, and file names the full output when it was cut. A non-zero exit is returned, not thrown.
- read(path, {offset, limit}) returns {lines, total, next}. Each line is `N:hhh|text`, where `N:hhh` is its anchor. It gives up to 2000 lines from offset (1-based); next is the offset to continue from, or null.
- edit(path, edits) changes lines of an existing file. Each edit is {op: \"replace\" | \"insert_after\" | \"delete\", from, to, lines}. from and to are lines or anchors from your latest read or edit of that file; to defaults to from; lines are the new lines; insert_after without from inserts at the top. Edits apply together or not at all, and a line that changed since it was read rejects them. Returns {total, preview}, where preview shows the changed lines with their new anchors.
- write(path, content) creates a file or replaces a whole file. Returns {created, lines}.
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
}

#[derive(Debug)]
pub(crate) struct Shelled {
    pub(crate) code: Option<i32>,
    pub(crate) output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bash,
    Read,
    Edit,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub kind: Kind,
    pub target: Box<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Started(Call),
    Failed(Box<str>),
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
        }
    }

    pub(crate) fn error(output: impl Into<Box<str>>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
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

    pub(crate) async fn run(&mut self, run: &Run, on_step: &mut dyn FnMut(Step)) -> Outcome {
        while self.steps.try_recv().is_ok() {}
        let (script, note) = match self.script.take() {
            Some(script) => (script, None),
            None => match Script::new(Rc::clone(&self.host)).await {
                Ok(script) => (script, self.had_sandbox.then_some(NEW_SANDBOX)),
                Err(err) => return Outcome::error(format!("could not start the sandbox: {err}")),
            },
        };
        self.had_sandbox = true;
        let finished = {
            let eval = tokio::time::timeout(SCRIPT_LIMIT, script.eval(&run.code));
            tokio::pin!(eval);
            loop {
                tokio::select! {
                    finished = &mut eval => break finished,
                    Some(step) = self.steps.recv() => on_step(step),
                }
            }
        };
        while let Ok(step) = self.steps.try_recv() {
            on_step(step);
        }
        let outcome = match finished {
            Ok(outcome) => {
                self.script = Some(script);
                outcome
            }
            Err(_) => Outcome::error(format!(
                "stopped: the run took over {} minutes, and its calls were killed",
                SCRIPT_LIMIT.as_secs() / 60
            )),
        };
        let text = match note {
            Some(note) => format!("{note}\n{}", outcome.output),
            None => outcome.output.into(),
        };
        Outcome {
            output: spill::fit(text, self.host.spill()).into(),
            is_error: outcome.is_error,
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
    use super::*;

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
    async fn steps_are_reported_while_the_script_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut steps = Vec::new();
        runner(dir.path())
            .run(
                &run("await bash`true`; await write('a.txt', 'x')"),
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
                })
            ]
        );
    }

    #[tokio::test]
    async fn sandbox_lost_to_a_cancel_is_announced_in_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut runner = runner(dir.path());
        runner.run(&run("let kept = 1"), &mut |_| {}).await;
        let cancelled = tokio::time::timeout(
            Duration::from_millis(300),
            runner.run(&run("await bash`sleep 5`"), &mut |_| {}),
        )
        .await;
        assert!(cancelled.is_err());

        let outcome = runner.run(&run("typeof kept"), &mut |_| {}).await;
        assert_eq!(outcome, Outcome::ok(format!("{NEW_SANDBOX}\nundefined")));
    }

    #[tokio::test]
    async fn cancel_kills_the_commands_of_the_script() {
        let dir = tempfile::tempdir().unwrap();
        let mut runner = runner(dir.path());
        let cancelled = tokio::time::timeout(
            Duration::from_millis(500),
            runner.run(&run("await bash`sleep 1; touch late`"), &mut |_| {}),
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
            .run(&run("'x'.repeat(100000)"), &mut |_| {})
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

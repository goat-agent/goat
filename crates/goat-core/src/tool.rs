mod bash;
mod danger;
mod files;
mod hashline;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::value::RawValue;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const SUMMARY_CHARS: usize = 100;

const READ_SCHEMA: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"File path, relative to the working directory or absolute"},"offset":{"type":["integer","null"],"description":"First line to show, 1-based; null starts at line 1"},"limit":{"type":["integer","null"],"description":"Maximum number of lines; null shows up to 2000"}},"required":["path","offset","limit"],"additionalProperties":false}"#;
const EDIT_SCHEMA: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"File to edit"},"edits":{"type":"array","description":"Edits addressed against the file as last read; applied together or not at all","items":{"type":"object","properties":{"op":{"type":"string","enum":["replace","insert_after","delete"]},"from":{"type":["string","null"],"description":"Anchor N:hhh of the first line of the range, or of the line to insert after; null with insert_after inserts at the top"},"to":{"type":["string","null"],"description":"Anchor of the last line of the range; null means the same line as from"},"lines":{"type":["array","null"],"items":{"type":"string"},"description":"New lines without line endings; null for delete"}},"required":["op","from","to","lines"],"additionalProperties":false}}},"required":["path","edits"],"additionalProperties":false}"#;
const WRITE_SCHEMA: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"File to create or replace"},"content":{"type":"string","description":"Complete file content"}},"required":["path","content"],"additionalProperties":false}"#;
const BASH_SCHEMA: &str = r#"{"type":"object","properties":{"command":{"type":"string","description":"Command for bash -c, run in the working directory"},"timeout":{"type":["integer","null"],"description":"Seconds before the command is killed; null means 120"}},"required":["command","timeout"],"additionalProperties":false}"#;

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

pub fn specs() -> Vec<Spec> {
    [
        (
            "read",
            "Read a text file. Every line is shown as `N:hhh|text`; `N:hhh` is that line's anchor for edit.",
            READ_SCHEMA,
        ),
        (
            "edit",
            "Change lines of an existing file by anchor. Use anchors from your latest read or edit result for that file. If a line changed since, nothing is written and the current lines are shown.",
            EDIT_SCHEMA,
        ),
        (
            "write",
            "Create a file or replace a whole file with new content.",
            WRITE_SCHEMA,
        ),
        (
            "bash",
            "Run a shell command with bash -c in the working directory: search with rg, list files, build, test, use git. Output is cut to its start and end when long; the full output is saved to a file named in the result.",
            BASH_SCHEMA,
        ),
    ]
    .into_iter()
    .map(|(name, description, schema)| Spec {
        name,
        description,
        schema: RawValue::from_string(schema.to_owned())
            .unwrap_or_else(|err| unreachable!("tool schema for {name} is valid json: {err}")),
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub output: Box<str>,
    pub is_error: bool,
}

impl Outcome {
    fn ok(output: impl Into<Box<str>>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    fn error(output: impl Into<Box<str>>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

#[derive(Debug)]
pub struct Toolbox {
    workspace: PathBuf,
    home: PathBuf,
    spill: PathBuf,
    danger: danger::Checker,
}

impl Toolbox {
    pub fn new(workspace: &Path, home: &Path, spill: &Path) -> Result<Self, Error> {
        Ok(Self {
            workspace: workspace.to_owned(),
            home: home.to_owned(),
            spill: spill.to_owned(),
            danger: danger::Checker::new()?,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub async fn run(&mut self, name: &str, input: &str) -> Outcome {
        let call = match Call::parse(name, input) {
            Ok(call) => call,
            Err(problem) => return Outcome::error(problem),
        };
        match call {
            Call::Read(args) => files::read(
                &self.resolve(&args.path),
                &args.path,
                args.offset,
                args.limit,
            ),
            Call::Edit(args) => files::edit(&self.resolve(&args.path), &args.path, args.edits),
            Call::Write(args) => files::write(&self.resolve(&args.path), &args.path, &args.content),
            Call::Bash(args) => {
                if let Some(reason) = self
                    .danger
                    .check(&args.command, &self.workspace, &self.home)
                {
                    return Outcome::error(format!(
                        "blocked: {reason}. goat never runs this; the user can run it themselves."
                    ));
                }
                let timeout =
                    Duration::from_secs(args.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1));
                bash::run(&args.command, &self.workspace, timeout, &self.spill).await
            }
        }
    }

    fn resolve(&self, raw: &str) -> PathBuf {
        match raw.strip_prefix('~') {
            Some("") => self.home.clone(),
            Some(rest) if rest.starts_with('/') => self.home.join(rest.trim_start_matches('/')),
            _ => self.workspace.join(raw),
        }
    }
}

pub fn summarize(name: &str, input: &str) -> String {
    #[derive(Deserialize)]
    struct Target {
        path: Option<String>,
        command: Option<String>,
    }

    let target = serde_json::from_str::<Target>(input).ok();
    let detail = target
        .and_then(|t| t.path.or(t.command))
        .and_then(|detail| detail.lines().next().map(str::to_owned))
        .unwrap_or_default();
    let mut summary = format!("{name} {detail}");
    if let Some((cut, _)) = summary.char_indices().nth(SUMMARY_CHARS) {
        summary.truncate(cut);
        summary.push('…');
    }
    summary
}

enum Call {
    Read(ReadArgs),
    Edit(EditArgs),
    Write(WriteArgs),
    Bash(BashArgs),
}

impl Call {
    fn parse(name: &str, input: &str) -> Result<Self, String> {
        let invalid = |err: serde_json::Error| format!("invalid arguments for {name}: {err}");
        match name {
            "read" => serde_json::from_str(input).map(Self::Read).map_err(invalid),
            "edit" => serde_json::from_str(input).map(Self::Edit).map_err(invalid),
            "write" => serde_json::from_str(input)
                .map(Self::Write)
                .map_err(invalid),
            "bash" => serde_json::from_str(input).map(Self::Bash).map_err(invalid),
            other => Err(format!(
                "there is no tool named {other}; use read, edit, write or bash"
            )),
        }
    }
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct EditArgs {
    path: String,
    edits: Vec<hashline::RawOp>,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct BashArgs {
    command: String,
    timeout: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolbox(dir: &Path) -> Toolbox {
        Toolbox::new(dir, Path::new("/home/me"), &dir.join(".spill")).unwrap()
    }

    #[test]
    fn specs_have_valid_schemas() {
        let specs = specs();
        assert_eq!(
            specs.iter().map(|s| s.name).collect::<Vec<_>>(),
            ["read", "edit", "write", "bash"]
        );
        for spec in specs {
            serde_json::from_str::<serde_json::Value>(spec.schema.get()).unwrap();
        }
    }

    #[tokio::test]
    async fn write_read_edit_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut tools = toolbox(dir.path());
        tools
            .run(
                "write",
                r#"{"path":"a.rs","content":"fn a() {}\nfn b() {}\n"}"#,
            )
            .await;
        let read = tools
            .run("read", r#"{"path":"a.rs","offset":null,"limit":null}"#)
            .await;
        let anchor = read
            .output
            .lines()
            .nth(1)
            .unwrap()
            .split('|')
            .next()
            .unwrap();
        let edit = format!(
            r#"{{"path":"a.rs","edits":[{{"op":"replace","from":"{anchor}","to":null,"lines":["fn b() {{ a() }}"]}}]}}"#
        );
        let outcome = tools.run("edit", &edit).await;
        assert!(!outcome.is_error, "{}", outcome.output);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.rs")).unwrap(),
            "fn a() {}\nfn b() { a() }\n"
        );
    }

    #[tokio::test]
    async fn dangerous_bash_is_blocked_before_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = toolbox(dir.path())
            .run("bash", r#"{"command":"rm -rf ~","timeout":null}"#)
            .await;
        assert!(outcome.is_error);
        assert!(outcome.output.starts_with("blocked:"));
    }

    #[tokio::test]
    async fn unknown_tool_is_an_error_result() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = toolbox(dir.path()).run("browse", "{}").await;
        assert!(outcome.is_error);
    }

    #[test]
    fn summary_shows_tool_and_target() {
        assert_eq!(
            summarize("bash", r#"{"command":"cargo test\necho"}"#),
            "bash cargo test"
        );
        assert_eq!(
            summarize("read", r#"{"path":"src/lib.rs"}"#),
            "read src/lib.rs"
        );
    }
}

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use goat_protocol::{ProcessState, RunId, ToolDisplay};
use goat_tool::{
    SandboxPolicy, Tool, ToolContext, ToolDefinitionContext, ToolError, ToolFuture, ToolInvocation,
    ToolOutput, ToolSpec, display,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::BashTool;

pub type BackgroundFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

pub struct ProcessStart {
    pub label: &'static str,
    pub command: String,
    pub name: Option<String>,
    pub cwd: PathBuf,
    pub watch: bool,
}

pub struct ProcessChunk {
    pub text: String,
    pub state: ProcessState,
    pub exit_code: Option<i32>,
}

pub trait BackgroundProcessService: Send + Sync {
    fn start<'a>(
        &'a self,
        request: ProcessStart,
        cancellation: &'a CancellationToken,
    ) -> BackgroundFuture<'a, RunId>;
    fn output(&self, run: RunId) -> BackgroundFuture<'_, ProcessChunk>;
    fn input(&self, run: RunId, text: String) -> BackgroundFuture<'_, ()>;
    fn kill(&self, run: RunId) -> BackgroundFuture<'_, ()>;
}

pub struct BackgroundBashTool {
    service: Arc<dyn BackgroundProcessService>,
}

impl BackgroundBashTool {
    pub fn new(service: Arc<dyn BackgroundProcessService>) -> Self {
        Self { service }
    }
}

#[derive(Deserialize)]
struct StartInput {
    command: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    watch: bool,
}

impl Tool for BackgroundBashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command via `sh -c` in the session directory and return its combined output. A nonzero exit code is reported in the output, not as an error. Set background=true to start it in the background instead: the call returns a run id immediately and a fresh turn wakes you when the command exits, so if you are only waiting for it, end your turn rather than polling. Read buffered output meanwhile with BashOutput, answer a prompt with BashInput, stop it with BashKill. Add watch=true to also be woken while it is still running, every time it prints output you have not read."
    }

    fn parameters(&self) -> serde_json::Value {
        BashTool.parameters()
    }

    fn definition(&self, context: ToolDefinitionContext) -> Option<ToolSpec> {
        let mut parameters = self.parameters();
        if context.top_level
            && let Some(properties) = parameters
                .get_mut("properties")
                .and_then(serde_json::Value::as_object_mut)
        {
            {
                properties.insert(
                    "background".to_owned(),
                    serde_json::json!({
                        "type": "boolean",
                        "description": "run in the background and return a run id instead of the output (default false)"
                    }),
                );
                properties.insert(
                    "watch".to_owned(),
                    serde_json::json!({
                        "type": "boolean",
                        "description": "with background, also wake on new output, not just on exit (default false)"
                    }),
                );
                properties.insert(
                    "name".to_owned(),
                    serde_json::json!({
                        "type": "string",
                        "description": "with background, a short name for the run"
                    }),
                );
            }
        }
        Some(ToolSpec {
            name: self.name(),
            description: if context.top_level {
                self.description()
            } else {
                BashTool.description()
            }
            .to_owned(),
            parameters,
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<StartInput>(input) {
            Ok(args) if args.background => ToolDisplay::primary(format!(
                "Bash(background, {})",
                process_start_summary(&args.command)
            )),
            _ => BashTool.display_input(input),
        }
    }

    fn run<'a>(&'a self, input: &'a str, context: &'a ToolContext) -> ToolFuture<'a> {
        BashTool.run(input, context)
    }

    fn invoke<'a>(
        &'a self,
        input: &'a str,
        context: &'a ToolContext,
        invocation: ToolInvocation<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: StartInput = serde_json::from_str(input)?;
            if !args.background {
                return BashTool.run(input, context).await;
            }
            if !invocation.definition_context.top_level {
                return Err(ToolError::policy(
                    "background processes are unavailable in this execution scope",
                ));
            }
            if !matches!(context.exec_policy, SandboxPolicy::Full) {
                return Err(ToolError::policy(
                    "background runs are only available with full shell access, not while planning",
                ));
            }
            let command = args.command.clone();
            let id = self
                .service
                .start(
                    ProcessStart {
                        label: "bash",
                        command: args.command,
                        name: args.name,
                        cwd: context.cwd.clone(),
                        watch: args.watch,
                    },
                    invocation.cancellation,
                )
                .await
                .map_err(ToolError::execution)?;
            Ok(ToolOutput::text(start_reply(id))
                .with_summary(format!("#{id} {}", process_start_summary(&command))))
        })
    }
}

#[derive(Clone, Copy)]
enum Operation {
    Output,
    Input,
    Kill,
}

pub struct BackgroundProcessTool {
    service: Arc<dyn BackgroundProcessService>,
    operation: Operation,
}

impl BackgroundProcessTool {
    fn new(service: Arc<dyn BackgroundProcessService>, operation: Operation) -> Self {
        Self { service, operation }
    }
}

#[derive(Deserialize)]
struct RunRef {
    run: RunId,
}

#[derive(Deserialize)]
struct InputArgs {
    run: RunId,
    text: String,
}

impl Tool for BackgroundProcessTool {
    fn name(&self) -> &'static str {
        match self.operation {
            Operation::Output => "BashOutput",
            Operation::Input => "BashInput",
            Operation::Kill => "BashKill",
        }
    }

    fn description(&self) -> &'static str {
        match self.operation {
            Operation::Output => {
                "Read output produced by a background Bash run since the last read. Returns immediately with whatever is buffered, plus whether the run is still going or has exited. To wait for a result, end your turn: the run wakes you when it exits."
            }
            Operation::Input => {
                "Send keystrokes to a background Bash run's stdin. Include a trailing newline to submit a line."
            }
            Operation::Kill => "Terminate a background Bash run and its process group.",
        }
    }

    fn parameters(&self) -> serde_json::Value {
        match self.operation {
            Operation::Input => serde_json::json!({
                "type": "object",
                "properties": {
                    "run": {"type": "string", "description": "run id from Bash(background=true)"},
                    "text": {"type": "string", "description": "raw bytes to write to stdin"}
                },
                "required": ["run", "text"]
            }),
            Operation::Output | Operation::Kill => serde_json::json!({
                "type": "object",
                "properties": {
                    "run": {"type": "string", "description": "run id from Bash(background=true)"}
                },
                "required": ["run"]
            }),
        }
    }

    fn enabled(&self, context: ToolDefinitionContext) -> bool {
        context.top_level
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<RunRef>(input) {
            Ok(args) => ToolDisplay::primary(format!("{}(#{})", self.name(), args.run)),
            Err(_) => ToolDisplay::primary(self.name()),
        }
    }

    fn run<'a>(&'a self, _input: &'a str, _context: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async {
            Err(ToolError::execution(
                "background invocation context is unavailable",
            ))
        })
    }

    fn invoke<'a>(
        &'a self,
        input: &'a str,
        _context: &'a ToolContext,
        invocation: ToolInvocation<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            if invocation.cancellation.is_cancelled() {
                return Err(ToolError::execution("interrupted"));
            }
            match self.operation {
                Operation::Output => {
                    let args: RunRef = serde_json::from_str(input)?;
                    let chunk = self
                        .service
                        .output(args.run)
                        .await
                        .map_err(ToolError::execution)?;
                    Ok(ToolOutput::text(output_reply(args.run, &chunk)))
                }
                Operation::Input => {
                    let args: InputArgs = serde_json::from_str(input)?;
                    self.service
                        .input(args.run, args.text)
                        .await
                        .map_err(ToolError::execution)?;
                    Ok(ToolOutput::text(format!("Wrote to run #{}.", args.run)))
                }
                Operation::Kill => {
                    let args: RunRef = serde_json::from_str(input)?;
                    self.service
                        .kill(args.run)
                        .await
                        .map_err(ToolError::execution)?;
                    Ok(ToolOutput::text(format!("Killed run #{}.", args.run)))
                }
            }
        })
    }
}

pub fn all_with_background(service: Arc<dyn BackgroundProcessService>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(BackgroundBashTool::new(service.clone())),
        Box::new(BackgroundProcessTool::new(
            service.clone(),
            Operation::Output,
        )),
        Box::new(BackgroundProcessTool::new(
            service.clone(),
            Operation::Input,
        )),
        Box::new(BackgroundProcessTool::new(service, Operation::Kill)),
    ]
}

fn process_start_summary(text: &str) -> String {
    display::truncate_chars(&display::flatten(text), 60)
}

fn start_reply(id: RunId) -> String {
    format!(
        "Started run #{id}. A fresh turn will wake you when it exits, so if you are only waiting for it, end your turn now instead of calling BashOutput. Read buffered output any time with BashOutput(run={id}); stop it with BashKill(run={id})."
    )
}

fn output_reply(id: RunId, chunk: &ProcessChunk) -> String {
    let status = match chunk.state {
        ProcessState::Running => "running".to_owned(),
        ProcessState::Exited => match chunk.exit_code {
            Some(code) => format!("exited (code {code})"),
            None => "exited".to_owned(),
        },
    };
    let waiting = match chunk.state {
        ProcessState::Exited => "",
        ProcessState::Running => {
            " — a fresh turn will wake you when it exits, so end your turn rather than reading it again to wait"
        }
    };
    if chunk.text.trim().is_empty() {
        format!("[no new output] run #{id} is {status}{waiting}")
    } else {
        let body = goat_tool::truncate(chunk.text.trim_end().to_owned(), 64 * 1024);
        format!("{body}\n[run #{id} is {status}{waiting}]")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use goat_protocol::{ProcessState, RunId};
    use goat_tool::{Tool, ToolDefinitionContext};

    use super::{
        BackgroundBashTool, BackgroundFuture, BackgroundProcessService, ProcessChunk, ProcessStart,
        StartInput, output_reply, start_reply,
    };

    struct Unavailable;

    impl BackgroundProcessService for Unavailable {
        fn start<'a>(
            &'a self,
            _request: ProcessStart,
            _cancellation: &'a tokio_util::sync::CancellationToken,
        ) -> BackgroundFuture<'a, RunId> {
            Box::pin(async { Err("unavailable".to_owned()) })
        }

        fn output(&self, _run: RunId) -> BackgroundFuture<'_, ProcessChunk> {
            Box::pin(async { Err("unavailable".to_owned()) })
        }

        fn input(&self, _run: RunId, _text: String) -> BackgroundFuture<'_, ()> {
            Box::pin(async { Err("unavailable".to_owned()) })
        }

        fn kill(&self, _run: RunId) -> BackgroundFuture<'_, ()> {
            Box::pin(async { Err("unavailable".to_owned()) })
        }
    }

    fn chunk(text: &str, state: ProcessState) -> ProcessChunk {
        ProcessChunk {
            text: text.to_owned(),
            state,
            exit_code: match state {
                ProcessState::Exited => Some(0),
                ProcessState::Running => None,
            },
        }
    }

    #[test]
    fn top_level_definition_adds_background_switches_without_polling_advice() {
        let tool = BackgroundBashTool::new(Arc::new(Unavailable));
        let definition = tool
            .definition(ToolDefinitionContext {
                interactive: true,
                top_level: true,
                planning: false,
            })
            .unwrap();
        let properties = definition.parameters.get("properties").unwrap();
        assert!(properties.get("command").is_some());
        assert!(properties.get("background").is_some());
        assert!(properties.get("watch").is_some());
        assert!(!definition.description.to_lowercase().contains("poll with"));
    }

    #[test]
    fn only_an_explicit_background_flag_detaches() {
        let enabled: StartInput =
            serde_json::from_str(r#"{"command":"sleep 1","background":true}"#).unwrap();
        let disabled: StartInput =
            serde_json::from_str(r#"{"command":"sleep 1","background":false}"#).unwrap();
        let absent: StartInput = serde_json::from_str(r#"{"command":"sleep 1"}"#).unwrap();
        assert!(enabled.background);
        assert!(!disabled.background);
        assert!(!absent.background);
    }

    #[test]
    fn start_reply_sends_a_waiting_agent_to_end_its_turn() {
        let reply = start_reply(RunId(3));
        assert!(reply.contains("end your turn"));
        assert!(!reply.contains("watch"));
    }

    #[test]
    fn running_run_reply_always_points_away_from_polling() {
        let quiet = output_reply(RunId(3), &chunk("", ProcessState::Running));
        assert!(quiet.contains("[no new output]"));
        assert!(quiet.contains("end your turn"));
        let chatty = output_reply(RunId(3), &chunk("compiling...\n", ProcessState::Running));
        assert!(chatty.contains("compiling..."));
        assert!(chatty.contains("end your turn"));
    }

    #[test]
    fn exited_run_reply_carries_no_waiting_advice() {
        let reply = output_reply(RunId(3), &chunk("done\n", ProcessState::Exited));
        assert!(reply.contains("exited (code 0)"));
        assert!(!reply.contains("end your turn"));
    }

    #[test]
    fn huge_output_keeps_its_status_marker() {
        let text = "x".repeat(200 * 1024);
        let reply = output_reply(RunId(3), &chunk(&text, ProcessState::Running));
        assert!(reply.contains("[output truncated]"));
        assert!(reply.trim_end().ends_with("to wait]"));
    }
}

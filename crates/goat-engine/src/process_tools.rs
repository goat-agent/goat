use std::fmt::Write as _;

use goat_tool::{SandboxPolicy, ToolOutput};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{Ctx, LoopEnv};

pub(crate) const PROCESS_START_TOOL_NAME: &str = "ProcessStart";
pub(crate) const PROCESS_OUTPUT_TOOL_NAME: &str = "ProcessOutput";
pub(crate) const PROCESS_INPUT_TOOL_NAME: &str = "ProcessInput";
pub(crate) const PROCESS_KILL_TOOL_NAME: &str = "ProcessKill";
pub(crate) const PROCESS_LIST_TOOL_NAME: &str = "ProcessList";
pub(crate) const PROCESS_WATCH_TOOL_NAME: &str = "ProcessWatch";

pub(crate) fn is_process_tool(name: &str) -> bool {
    matches!(
        name,
        PROCESS_START_TOOL_NAME
            | PROCESS_OUTPUT_TOOL_NAME
            | PROCESS_INPUT_TOOL_NAME
            | PROCESS_KILL_TOOL_NAME
            | PROCESS_LIST_TOOL_NAME
            | PROCESS_WATCH_TOOL_NAME
    )
}

pub(crate) fn tool_defs() -> Vec<goat_provider::ToolDefinition> {
    vec![
        def(
            PROCESS_START_TOOL_NAME,
            "Start a long-running command in the background and return immediately with a process id. Use this for dev servers (pnpm dev, vite), watchers, or a poller that waits for a long task (e.g. `gh run watch`) instead of blocking on Bash. Output is buffered; read it later with ProcessOutput. If you want the result within this same turn, leave watch off and poll with ProcessOutput. Set watch=true only to be woken by a *future* event once this turn has already ended and you are idle: after it exits or prints new output you have not yet read, a fresh turn wakes you (pipe the command through grep to keep those wakes meaningful). Output you already read with ProcessOutput never wakes you again. The process keeps running across turns until it exits or you call ProcessKill.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "shell command to run in the background"},
                    "watch": {"type": "boolean", "description": "wake the agent on new output and on exit (default false)"}
                },
                "required": ["command"]
            }),
        ),
        def(
            PROCESS_OUTPUT_TOOL_NAME,
            "Read output produced by a background process since the last read (a moving cursor, not the whole history). Returns whether the process is still running or has exited with its code.",
            serde_json::json!({
                "type": "object",
                "properties": {"process": {"type": "string", "description": "process id from ProcessStart"}},
                "required": ["process"]
            }),
        ),
        def(
            PROCESS_INPUT_TOOL_NAME,
            "Send keystrokes to a background process's stdin (e.g. answer an interactive prompt). Include a trailing newline to submit a line.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "process": {"type": "string", "description": "process id from ProcessStart"},
                    "text": {"type": "string", "description": "raw bytes to write to stdin"}
                },
                "required": ["process", "text"]
            }),
        ),
        def(
            PROCESS_KILL_TOOL_NAME,
            "Terminate a background process (and its process group).",
            serde_json::json!({
                "type": "object",
                "properties": {"process": {"type": "string", "description": "process id from ProcessStart"}},
                "required": ["process"]
            }),
        ),
        def(
            PROCESS_LIST_TOOL_NAME,
            "List background processes and their state (running or exited).",
            serde_json::json!({"type": "object", "properties": {}}),
        ),
        def(
            PROCESS_WATCH_TOOL_NAME,
            "Turn push observation on or off for a background process. When on, a future exit or unread new output wakes you in a fresh turn once you are idle — output you already read with ProcessOutput does not. When off, output is only buffered for ProcessOutput.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "process": {"type": "string", "description": "process id from ProcessStart"},
                    "on": {"type": "boolean", "description": "true to watch, false to stop watching"}
                },
                "required": ["process", "on"]
            }),
        ),
    ]
}

fn def(name: &str, description: &str, schema: serde_json::Value) -> goat_provider::ToolDefinition {
    goat_provider::ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema: schema,
    }
}

pub(crate) fn call_display(name: &str, input: &str) -> goat_protocol::ToolDisplay {
    let detail = process_id_arg(input).map(|p| format!("#{p}"));
    match (name, detail) {
        (PROCESS_START_TOOL_NAME, _) => {
            let cmd = serde_json::from_str::<StartInput>(input)
                .map(|i| i.command)
                .unwrap_or_default();
            goat_protocol::ToolDisplay::primary(format!(
                "ProcessStart({})",
                process_start_summary(&cmd)
            ))
        }
        (_, Some(detail)) => goat_protocol::ToolDisplay::primary(format!("{name}({detail})")),
        (_, None) => goat_protocol::ToolDisplay::primary(name.to_owned()),
    }
}

fn process_start_summary(text: &str) -> String {
    goat_tool::display::truncate_chars(&goat_tool::display::flatten(text), 60)
}

fn process_id_arg(input: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let raw = value.get("process")?;
    raw.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| raw.as_u64())
}

#[derive(Deserialize)]
struct StartInput {
    command: String,
    #[serde(default)]
    watch: bool,
}

#[derive(Deserialize)]
struct ProcessRef {
    process: goat_protocol::ProcessId,
}

#[derive(Deserialize)]
struct InputArgs {
    process: goat_protocol::ProcessId,
    text: String,
}

#[derive(Deserialize)]
struct WatchArgs {
    process: goat_protocol::ProcessId,
    on: bool,
}

pub(crate) async fn run_process_tool(
    ctx: &Ctx<'_>,
    env: &LoopEnv<'_>,
    name: &str,
    input_json: &str,
    token: &CancellationToken,
) -> Option<Result<ToolOutput, String>> {
    if token.is_cancelled() {
        return None;
    }
    let result = match name {
        PROCESS_START_TOOL_NAME => start(ctx, env, input_json).await,
        PROCESS_OUTPUT_TOOL_NAME => output(ctx, input_json).await,
        PROCESS_INPUT_TOOL_NAME => input(ctx, input_json).await,
        PROCESS_KILL_TOOL_NAME => kill(ctx, input_json).await,
        PROCESS_LIST_TOOL_NAME => Ok(list(ctx).await),
        PROCESS_WATCH_TOOL_NAME => watch(ctx, input_json).await,
        _ => Err(format!("unknown process tool: {name}")),
    };
    Some(result)
}

async fn start(ctx: &Ctx<'_>, env: &LoopEnv<'_>, input_json: &str) -> Result<ToolOutput, String> {
    if !matches!(env.exec_policy, SandboxPolicy::Full) {
        return Err(
            "background processes are only available with full shell access, not while planning"
                .to_owned(),
        );
    }
    let args: StartInput =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    let started = ctx
        .processes
        .spawn(&args.command, env.cwd, args.watch)
        .await
        .map_err(|err| err.to_string())?;
    if let Some(pgid) = started.pgid {
        let db_id = ctx
            .store
            .create_process(goat_store::NewProcess {
                pgid: i64::from(pgid),
                command: args.command.clone(),
                cwd: env.cwd.display().to_string(),
                started_at: crate::persist::now_ms(),
            })
            .await
            .ok();
        if let Some(db_id) = db_id {
            ctx.processes.set_db_id(started.id, db_id).await;
        }
    }
    let id = started.id;
    let watched = if args.watch { " (watched)" } else { "" };
    Ok(ToolOutput::text(format!(
        "Started process #{id}{watched}. Read output with ProcessOutput(process={id}); stop with ProcessKill(process={id})."
    ))
    .with_summary(format!("#{id} {}", process_start_summary(&args.command))))
}

async fn output(ctx: &Ctx<'_>, input_json: &str) -> Result<ToolOutput, String> {
    let args: ProcessRef =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    let chunk = ctx
        .processes
        .read_new(args.process)
        .await
        .ok_or_else(|| format!("no process #{}", args.process))?;
    let status = match chunk.state {
        goat_protocol::ProcessState::Running => "running".to_owned(),
        goat_protocol::ProcessState::Exited => match chunk.exit_code {
            Some(code) => format!("exited (code {code})"),
            None => "exited".to_owned(),
        },
    };
    let body = if chunk.text.trim().is_empty() {
        format!("[no new output] process #{} is {status}", args.process)
    } else {
        format!(
            "{}\n[process #{} is {status}]",
            chunk.text.trim_end(),
            args.process
        )
    };
    Ok(ToolOutput::text(crate::tools_exec::cap_tool_result(body)))
}

async fn input(ctx: &Ctx<'_>, input_json: &str) -> Result<ToolOutput, String> {
    let args: InputArgs =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    ctx.processes.write_stdin(args.process, &args.text).await?;
    Ok(ToolOutput::text(format!(
        "Wrote to process #{}.",
        args.process
    )))
}

async fn kill(ctx: &Ctx<'_>, input_json: &str) -> Result<ToolOutput, String> {
    let args: ProcessRef =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    ctx.processes.kill(args.process).await?;
    Ok(ToolOutput::text(format!(
        "Killed process #{}.",
        args.process
    )))
}

async fn watch(ctx: &Ctx<'_>, input_json: &str) -> Result<ToolOutput, String> {
    let args: WatchArgs =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    ctx.processes.set_watch(args.process, args.on).await?;
    let state = if args.on { "watching" } else { "not watching" };
    Ok(ToolOutput::text(format!(
        "Now {state} process #{}.",
        args.process
    )))
}

async fn list(ctx: &Ctx<'_>) -> ToolOutput {
    let processes = ctx.processes.list().await;
    if processes.is_empty() {
        return ToolOutput::text("No background processes.".to_owned());
    }
    let mut out = String::from("Background processes:\n");
    for p in &processes {
        let state = match p.state {
            goat_protocol::ProcessState::Running => "running".to_owned(),
            goat_protocol::ProcessState::Exited => match p.exit_code {
                Some(code) => format!("exited({code})"),
                None => "exited".to_owned(),
            },
        };
        let watched = if p.watched { " watched" } else { "" };
        let _ = writeln!(out, "  #{} [{state}{watched}] {}", p.id, p.command);
    }
    ToolOutput::text(out)
}

pub(crate) async fn roster_message(ctx: &Ctx<'_>) -> Option<goat_provider::Message> {
    let processes = ctx.processes.list().await;
    let running: Vec<_> = processes
        .iter()
        .filter(|p| p.state == goat_protocol::ProcessState::Running)
        .collect();
    if running.is_empty() {
        return None;
    }
    let mut text = String::from(
        "<environment-status>\nAutomated status snapshot, not a user message — background processes running now (read with ProcessOutput, stop with ProcessKill):\n",
    );
    for p in running {
        let watched = if p.watched { " watched" } else { "" };
        let _ = writeln!(text, "  #{}{watched} — {}", p.id, p.command);
    }
    text.push_str("</environment-status>");
    Some(goat_provider::Message::text(
        goat_provider::MessageRole::User,
        text,
    ))
}

use std::fmt::Write as _;

use goat_tool::{SandboxPolicy, ToolOutput};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{Ctx, LoopEnv};

pub(crate) const BASH_TOOL_NAME: &str = "Bash";
pub(crate) const BASH_OUTPUT_TOOL_NAME: &str = "BashOutput";
pub(crate) const BASH_INPUT_TOOL_NAME: &str = "BashInput";
pub(crate) const BASH_KILL_TOOL_NAME: &str = "BashKill";

const BACKGROUND_NOTE: &str = " Set background=true to start it in the background instead: the call returns a run id immediately and a fresh turn wakes you when the command exits, so if you are only waiting for it, end your turn rather than polling. Read buffered output meanwhile with BashOutput, answer a prompt with BashInput, stop it with BashKill. Add watch=true to also be woken while it is still running, every time it prints output you have not read (log monitoring; pipe through grep to keep those wakes meaningful).";

pub(crate) fn is_bash_run_tool(name: &str) -> bool {
    matches!(
        name,
        BASH_OUTPUT_TOOL_NAME | BASH_INPUT_TOOL_NAME | BASH_KILL_TOOL_NAME
    )
}

pub(crate) fn augment_bash(def: &mut goat_provider::ToolDefinition) {
    def.description.push_str(BACKGROUND_NOTE);
    let Some(properties) = def
        .input_schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
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
}

pub(crate) fn wants_background(input_json: &str) -> bool {
    serde_json::from_str::<StartInput>(input_json).is_ok_and(|args| args.background)
}

pub(crate) fn tool_defs() -> Vec<goat_provider::ToolDefinition> {
    vec![
        def(
            BASH_OUTPUT_TOOL_NAME,
            "Read output produced by a background Bash run since the last read (a moving cursor, not the whole history). Returns immediately with whatever is buffered, plus whether the run is still going or has exited with its code. This is a snapshot for when you have other work in flight — it does not wait. To wait for a result, end your turn: the run wakes you when it exits. Reading an exit here counts as seeing it, so it will not wake you again.",
            serde_json::json!({
                "type": "object",
                "properties": {"run": {"type": "string", "description": "run id from Bash(background=true)"}},
                "required": ["run"]
            }),
        ),
        def(
            BASH_INPUT_TOOL_NAME,
            "Send keystrokes to a background Bash run's stdin (e.g. answer an interactive prompt). Include a trailing newline to submit a line.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "run": {"type": "string", "description": "run id from Bash(background=true)"},
                    "text": {"type": "string", "description": "raw bytes to write to stdin"}
                },
                "required": ["run", "text"]
            }),
        ),
        def(
            BASH_KILL_TOOL_NAME,
            "Terminate a background Bash run (and its process group).",
            serde_json::json!({
                "type": "object",
                "properties": {"run": {"type": "string", "description": "run id from Bash(background=true)"}},
                "required": ["run"]
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
    match run_id_arg(input) {
        Some(run) => goat_protocol::ToolDisplay::primary(format!("{name}(#{run})")),
        None => goat_protocol::ToolDisplay::primary(name.to_owned()),
    }
}

pub(crate) fn background_start_display(input: &str) -> goat_protocol::ToolDisplay {
    let command = serde_json::from_str::<StartInput>(input)
        .map(|i| i.command)
        .unwrap_or_default();
    goat_protocol::ToolDisplay::primary(format!(
        "Bash(background, {})",
        process_start_summary(&command)
    ))
}

fn process_start_summary(text: &str) -> String {
    goat_tool::display::truncate_chars(&goat_tool::display::flatten(text), 60)
}

fn run_id_arg(input: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let raw = value.get("run")?;
    raw.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| raw.as_u64())
}

#[derive(Deserialize)]
struct StartInput {
    command: String,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    watch: bool,
}

#[derive(Deserialize)]
struct RunRef {
    run: goat_protocol::RunId,
}

#[derive(Deserialize)]
struct InputArgs {
    run: goat_protocol::RunId,
    text: String,
}

pub(crate) async fn run_bash_run_tool(
    ctx: &Ctx,
    name: &str,
    input_json: &str,
    token: &CancellationToken,
) -> Option<Result<ToolOutput, String>> {
    if token.is_cancelled() {
        return None;
    }
    let result = match name {
        BASH_OUTPUT_TOOL_NAME => output(ctx, input_json).await,
        BASH_INPUT_TOOL_NAME => input(ctx, input_json).await,
        BASH_KILL_TOOL_NAME => kill(ctx, input_json).await,
        _ => Err(format!("unknown background tool: {name}")),
    };
    Some(result)
}

pub(crate) async fn start_background(
    ctx: &Ctx,
    env: &LoopEnv,
    input_json: &str,
    token: &CancellationToken,
) -> Option<Result<ToolOutput, String>> {
    if token.is_cancelled() {
        return None;
    }
    Some(start(ctx, env, input_json).await)
}

async fn start(ctx: &Ctx, env: &LoopEnv, input_json: &str) -> Result<ToolOutput, String> {
    if !matches!(env.exec_policy, SandboxPolicy::Full) {
        return Err(
            "background runs are only available with full shell access, not while planning"
                .to_owned(),
        );
    }
    let args: StartInput =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    let started = ctx
        .background
        .spawn(&args.command, &env.cwd, args.watch)
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
            ctx.background.set_db_id(started.id, db_id).await;
        }
    }
    let id = started.id;
    Ok(ToolOutput::text(start_reply(id))
        .with_summary(format!("#{id} {}", process_start_summary(&args.command))))
}

fn start_reply(id: goat_protocol::RunId) -> String {
    format!(
        "Started run #{id}. A fresh turn will wake you when it exits, so if you are only waiting for it, end your turn now instead of calling BashOutput. Read buffered output any time with BashOutput(run={id}); stop it with BashKill(run={id})."
    )
}

async fn output(ctx: &Ctx, input_json: &str) -> Result<ToolOutput, String> {
    let args: RunRef =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    let chunk = ctx
        .background
        .read_new(args.run)
        .await
        .ok_or_else(|| format!("no run #{}", args.run))?;
    Ok(ToolOutput::text(output_reply(args.run, &chunk)))
}

fn output_reply(id: goat_protocol::RunId, chunk: &crate::background::ReadChunk) -> String {
    let status = match chunk.state {
        goat_protocol::ProcessState::Running => "running".to_owned(),
        goat_protocol::ProcessState::Exited => match chunk.exit_code {
            Some(code) => format!("exited (code {code})"),
            None => "exited".to_owned(),
        },
    };
    let waiting = match chunk.state {
        goat_protocol::ProcessState::Exited => "",
        goat_protocol::ProcessState::Running => {
            " — a fresh turn will wake you when it exits, so end your turn rather than reading it again to wait"
        }
    };
    if chunk.text.trim().is_empty() {
        format!("[no new output] run #{id} is {status}{waiting}")
    } else {
        let body = crate::tools_exec::cap_tool_result(chunk.text.trim_end().to_owned());
        format!("{body}\n[run #{id} is {status}{waiting}]")
    }
}

async fn input(ctx: &Ctx, input_json: &str) -> Result<ToolOutput, String> {
    let args: InputArgs =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    ctx.background.write_stdin(args.run, &args.text).await?;
    Ok(ToolOutput::text(format!("Wrote to run #{}.", args.run)))
}

async fn kill(ctx: &Ctx, input_json: &str) -> Result<ToolOutput, String> {
    let args: RunRef =
        serde_json::from_str(input_json).map_err(|err| format!("invalid input: {err}"))?;
    ctx.background
        .kill(args.run, Some(crate::background::Kind::Bash))
        .await?;
    Ok(ToolOutput::text(format!("Killed run #{}.", args.run)))
}

pub(crate) async fn roster_message(ctx: &Ctx) -> Option<goat_provider::Message> {
    let running = ctx.background.roster().await;
    if running.is_empty() {
        return None;
    }
    Some(goat_provider::Message::text(
        goat_provider::MessageRole::User,
        roster_text(&running),
    ))
}

fn roster_text(running: &[crate::background::RunInfo]) -> String {
    let mut text = String::from(
        "<environment-status>\nAutomated status snapshot, not a user message — background work going now (a bash run reads with BashOutput and stops with BashKill; a subagent reports on its own when it finishes and stops with SubagentKill):\n",
    );
    for run in running {
        let watched = if run.watched { " watched" } else { "" };
        let _ = writeln!(
            text,
            "  #{}{watched} — {}: {}",
            run.id,
            run.kind.label(),
            run.title
        );
    }
    text.push_str("</environment-status>");
    text
}

#[cfg(test)]
mod tests {
    use super::{augment_bash, output_reply, start_reply, tool_defs, wants_background};
    use crate::background::ReadChunk;
    use goat_protocol::{ProcessState, RunId};

    fn chunk(text: &str, state: ProcessState) -> ReadChunk {
        ReadChunk {
            text: text.to_owned(),
            state,
            exit_code: match state {
                ProcessState::Exited => Some(0),
                ProcessState::Running => None,
            },
        }
    }

    fn bash_def() -> goat_provider::ToolDefinition {
        goat_provider::ToolDefinition {
            name: "Bash".to_owned(),
            description: "Run a shell command.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }),
        }
    }

    #[test]
    fn start_reply_sends_a_waiting_agent_to_end_its_turn() {
        let reply = start_reply(RunId(3));
        assert!(reply.contains("end your turn"), "got: {reply}");
        assert!(
            !reply.contains("watch"),
            "waiting must not require watching, got: {reply}"
        );
    }

    #[test]
    fn running_run_reply_always_points_away_from_polling() {
        let quiet = output_reply(RunId(3), &chunk("", ProcessState::Running));
        assert!(quiet.contains("[no new output]"), "got: {quiet}");
        assert!(quiet.contains("end your turn"), "got: {quiet}");

        let chatty = output_reply(RunId(3), &chunk("compiling...\n", ProcessState::Running));
        assert!(chatty.contains("compiling..."), "got: {chatty}");
        assert!(
            chatty.contains("end your turn"),
            "a run that keeps printing must still steer the agent away from polling, got: {chatty}"
        );
    }

    #[test]
    fn exited_run_reply_carries_no_waiting_advice() {
        let reply = output_reply(RunId(3), &chunk("done\n", ProcessState::Exited));
        assert!(reply.contains("exited (code 0)"), "got: {reply}");
        assert!(!reply.contains("end your turn"), "got: {reply}");
    }

    #[test]
    fn huge_output_keeps_its_status_marker() {
        let text = "x".repeat(200 * 1024);
        let reply = output_reply(RunId(3), &chunk(&text, ProcessState::Running));
        assert!(reply.contains("[output truncated]"), "should be capped");
        assert!(
            reply.trim_end().ends_with("to wait]"),
            "capping must not eat the status marker, tail was: {}",
            &reply[reply.len().saturating_sub(120)..]
        );
    }

    #[test]
    fn no_tool_description_tells_the_agent_to_poll() {
        let mut bash = bash_def();
        augment_bash(&mut bash);
        for def in tool_defs().iter().chain(std::iter::once(&bash)) {
            assert!(
                !def.description.to_lowercase().contains("poll with"),
                "{} still instructs polling: {}",
                def.name,
                def.description
            );
        }
    }

    #[test]
    fn augment_bash_adds_the_background_switches() {
        let mut def = bash_def();
        augment_bash(&mut def);
        let properties = def.input_schema.get("properties").expect("properties");
        assert!(properties.get("background").is_some());
        assert!(properties.get("watch").is_some());
        assert!(properties.get("command").is_some(), "command must survive");
        assert!(def.description.contains("background=true"), "described");
    }

    #[test]
    fn only_an_explicit_background_flag_detaches() {
        assert!(wants_background(
            r#"{"command":"sleep 1","background":true}"#
        ));
        assert!(!wants_background(
            r#"{"command":"sleep 1","background":false}"#
        ));
        assert!(!wants_background(r#"{"command":"sleep 1"}"#));
        assert!(!wants_background("not json"));
    }
}

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use goat_tool::command_safety::deny_reason;
use goat_tool::{
    ToolCall, ToolContent, ToolContext, ToolFactory, ToolHandler, ToolName, ToolOutput, ToolSpec,
};
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;
use tokio::time::timeout;

pub const NAME: ToolName = ToolName::from_static("shell");
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
const MAX_OUTPUT_CHARS: usize = 12_000;

pub struct ShellTool;

#[derive(Debug, Deserialize)]
struct ShellArgs {
    command: String,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[async_trait]
impl ToolHandler for ShellTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args = match serde_json::from_value::<ShellArgs>(call.arguments) {
            Ok(args) => args,
            Err(e) => return ToolOutput::error(format!("invalid shell input: {e}")),
        };
        if args.command.trim().is_empty() {
            return ToolOutput::error("command must not be empty");
        }
        if let Some(reason) = deny_reason(&args.command) {
            return ToolOutput::error(format!("command denied: {reason}"));
        }
        let cwd = match resolve_cwd(&ctx.goat_root, args.cwd.as_deref()) {
            Ok(cwd) => cwd,
            Err(e) => return ToolOutput::error(e),
        };
        let timeout_ms = args
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1_000, MAX_TIMEOUT_MS);

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-lc")
            .arg(&args.command)
            .current_dir(&cwd)
            .kill_on_drop(true);

        let output = match timeout(Duration::from_millis(timeout_ms), cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return ToolOutput::error(format!("shell failed to start: {e}")),
            Err(_) => {
                let stderr = format!("timed out after {timeout_ms}ms");
                return ToolOutput {
                    content: vec![ToolContent::Text {
                        text: shell_output_text(None, "", &stderr, true),
                    }],
                    structured_content: Some(json!({
                    "exit_code": null,
                    "stdout": "",
                    "stderr": stderr,
                    "timed_out": true,
                    })),
                    is_error: true,
                };
            }
        };
        let stdout = truncate(&String::from_utf8_lossy(&output.stdout));
        let stderr = truncate(&String::from_utf8_lossy(&output.stderr));
        let structured = json!({
            "exit_code": output.status.code(),
            "stdout": stdout,
            "stderr": stderr,
            "timed_out": false,
        });
        ToolOutput {
            content: vec![ToolContent::Text {
                text: shell_output_text(output.status.code(), &stdout, &stderr, false),
            }],
            structured_content: Some(structured),
            is_error: false,
        }
    }
}

fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        NAME,
        "Run a local shell command on the host. Use for build, test, inspect, and ordinary automation commands. Catastrophic destructive commands and sensitive paths are denied by the tool implementation.",
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to run." },
                "cwd": { "type": "string", "description": "Optional working directory. Relative paths resolve under the goat root." },
                "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 120000, "description": "Optional timeout in milliseconds." }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    );
    spec.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "exit_code": { "type": ["integer", "null"] },
            "stdout": { "type": "string" },
            "stderr": { "type": "string" },
            "timed_out": { "type": "boolean" }
        },
        "required": ["exit_code", "stdout", "stderr", "timed_out"],
        "additionalProperties": false
    }));
    spec
}

fn ctor() -> Arc<dyn ToolHandler> {
    Arc::new(ShellTool)
}

inventory::submit! {
    ToolFactory { name: NAME, default_enabled: true, spec, ctor }
}

fn resolve_cwd(goat_root: &Path, cwd: Option<&Path>) -> Result<PathBuf, String> {
    let base = cwd
        .map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                goat_root.join(p)
            }
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| goat_root.to_path_buf()));
    std::fs::canonicalize(&base).map_err(|e| format!("invalid cwd {}: {e}", base.display()))
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_OUTPUT_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_OUTPUT_CHARS).collect();
    out.push_str("\n...[truncated]");
    out
}

fn shell_output_text(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    timed_out: bool,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "exit_code: {}\n",
        exit_code.map_or_else(|| "null".to_string(), |code| code.to_string())
    ));
    out.push_str(&format!("timed_out: {timed_out}\n"));
    out.push_str("stdout:\n");
    out.push_str(&fenced_text(stdout));
    if !stderr.is_empty() {
        out.push_str("\nstderr:\n");
        out.push_str(&fenced_text(stderr));
    }
    out
}

fn fenced_text(text: &str) -> String {
    let fence = fence_for(text);
    if text.is_empty() {
        format!("{fence}text\n{fence}")
    } else {
        format!("{fence}text\n{text}\n{fence}")
    }
}

fn fence_for(text: &str) -> String {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            max_run = max_run.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(max_run.max(2) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_types::{ChannelId, ConversationId, InstanceId, PersonaId};

    fn ctx(root: PathBuf) -> ToolContext {
        ToolContext {
            persona: PersonaId::new(),
            conversation: ConversationId::new(ChannelId::new("test"), InstanceId::new(), "x"),
            goat_root: root,
            read_state: Default::default(),
        }
    }

    #[tokio::test]
    async fn runs_benign_command() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ShellTool;
        let out = tool
            .call(
                ctx(temp.path().to_path_buf()),
                ToolCall {
                    call_id: "c1".into(),
                    name: NAME,
                    arguments: json!({"command":"printf hello","cwd": temp.path().to_string_lossy()}),
                },
            )
            .await;
        assert!(!out.is_error);
        assert_eq!(out.structured_content.as_ref().unwrap()["stdout"], "hello");
        let text = out.text_for_model();
        assert!(text.contains("exit_code: 0"));
        assert!(text.contains("stdout:\n```text\nhello\n```"));
    }

    #[test]
    fn shell_output_text_preserves_newlines_for_model() {
        let text = shell_output_text(Some(0), "a\nb\n", "", false);
        assert!(text.contains("stdout:\n```text\na\nb\n\n```"));
        assert!(!text.contains("\\n"));
    }

    #[test]
    fn fence_for_uses_longer_fence_than_output_backticks() {
        let text = fenced_text("```inside```");
        assert!(text.starts_with("````text\n"));
        assert!(text.ends_with("\n````"));
    }
}

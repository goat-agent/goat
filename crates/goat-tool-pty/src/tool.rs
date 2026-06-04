use std::sync::Arc;

use async_trait::async_trait;
use goat_tool::{
    command_safety::deny_reason, ToolCall, ToolContent, ToolContext, ToolHandler, ToolName,
    ToolOutput, ToolSpec,
};
use serde::Deserialize;
use serde_json::json;

use crate::keys::PtyInputItem;
use crate::manager::PtyManager;

pub const NAME: ToolName = ToolName::from_static("pty");

pub struct PtyTool {
    pub manager: Arc<PtyManager>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Action {
    Open,
    Input,
    Read,
    List,
    Close,
    Resize,
    Signal,
}

#[derive(Deserialize)]
struct PtyArgs {
    action: Action,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    input: Option<Vec<PtyInputItem>>,
    #[serde(default)]
    signal: Option<String>,
}

#[async_trait]
impl ToolHandler for PtyTool {
    async fn call(&self, _ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: PtyArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid input: {e}")),
        };

        match args.action {
            Action::Open => {
                let cmd = args.command.as_deref().unwrap_or("");
                if !cmd.trim().is_empty() {
                    if let Some(reason) = deny_reason(cmd) {
                        return ToolOutput::error(format!("command denied: {reason}"));
                    }
                }
                match self.manager.open(cmd, args.rows, args.cols).await {
                    Ok((id, rows, cols)) => ToolOutput::structured(json!({
                        "session_id": id,
                        "status": "running",
                        "rows": rows,
                        "cols": cols,
                    })),
                    Err(e) => ToolOutput::error(e),
                }
            }

            Action::Input => {
                let Some(id) = args.session_id.as_deref() else {
                    return ToolOutput::error("input requires session_id");
                };
                let empty = vec![];
                let items = args.input.as_deref().unwrap_or(&empty);
                match self.manager.input(id, items).await {
                    Ok(n) => ToolOutput::text(format!("ok ({n} chunk(s))")),
                    Err(e) => ToolOutput::error(e),
                }
            }

            Action::Read => {
                let Some(id) = args.session_id.as_deref() else {
                    return ToolOutput::error("read requires session_id");
                };
                match self.manager.read(id) {
                    Ok(snap) => {
                        let fence = fence_for(&snap.screen);
                        let text = format!(
                            "session {} ({}) {}x{}  cursor({},{})\n{fence}text\n{}\n{fence}",
                            snap.session_id,
                            snap.status,
                            snap.rows,
                            snap.cols,
                            snap.cursor.0,
                            snap.cursor.1,
                            snap.screen,
                        );
                        ToolOutput {
                            content: vec![ToolContent::Text { text }],
                            structured_content: Some(json!({
                                "session_id": snap.session_id,
                                "status": snap.status.to_string(),
                                "rows": snap.rows,
                                "cols": snap.cols,
                                "cursor": [snap.cursor.0, snap.cursor.1],
                                "screen": snap.screen,
                            })),
                            is_error: false,
                        }
                    }
                    Err(e) => ToolOutput::error(e),
                }
            }

            Action::List => {
                let infos = self.manager.list();
                let text = if infos.is_empty() {
                    "no open sessions".to_string()
                } else {
                    let mut t = format!(
                        "{:<6} {:<30} {:<14} {:<9} {}\n",
                        "id", "command", "status", "size", "idle_ms"
                    );
                    for s in &infos {
                        t.push_str(&format!(
                            "{:<6} {:<30} {:<14} {}x{:<5} {}\n",
                            s.id, s.command, s.status, s.rows, s.cols, s.idle_ms
                        ));
                    }
                    t
                };
                let sessions: Vec<_> = infos
                    .iter()
                    .map(|s| {
                        json!({
                            "id": s.id,
                            "command": s.command,
                            "status": s.status.to_string(),
                            "rows": s.rows,
                            "cols": s.cols,
                            "idle_ms": s.idle_ms,
                        })
                    })
                    .collect();
                ToolOutput {
                    content: vec![ToolContent::Text { text }],
                    structured_content: Some(json!({ "sessions": sessions })),
                    is_error: false,
                }
            }

            Action::Close => {
                let Some(id) = args.session_id.as_deref() else {
                    return ToolOutput::error("close requires session_id");
                };
                match self.manager.close(id).await {
                    Ok(status) => ToolOutput::text(format!("closed {id} ({status})")),
                    Err(e) => ToolOutput::error(e),
                }
            }

            Action::Resize => {
                let Some(id) = args.session_id.as_deref() else {
                    return ToolOutput::error("resize requires session_id");
                };
                let (rows, cols) = (args.rows.unwrap_or(24), args.cols.unwrap_or(80));
                match self.manager.resize(id, rows, cols) {
                    Ok(()) => ToolOutput::text(format!("resized {id} to {rows}x{cols}")),
                    Err(e) => ToolOutput::error(e),
                }
            }

            Action::Signal => {
                let Some(id) = args.session_id.as_deref() else {
                    return ToolOutput::error("signal requires session_id");
                };
                let sig = args.signal.as_deref().unwrap_or("int");
                match self.manager.signal(id, sig).await {
                    Ok(()) => ToolOutput::text(format!("sent SIG{} to {id}", sig.to_uppercase())),
                    Err(e) => ToolOutput::error(e),
                }
            }
        }
    }
}

pub fn spec() -> ToolSpec {
    ToolSpec::new(
        NAME,
        "Open and interact with persistent PTY sessions (tmux, vim, claude, etc.) \
         that survive across agent turns. action:open launches a program via /bin/sh -c, \
         action:input sends keystrokes, action:read returns the rendered screen, \
         action:list/close/resize/signal manage sessions. \
         Launch commands are safety-checked; keystrokes are not.",
        json!({
            "type": "object",
            "required": ["action"],
            "additionalProperties": false,
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "input", "read", "list", "close", "resize", "signal"]
                },
                "session_id": { "type": "string" },
                "command": {
                    "type": "string",
                    "description": "Program to launch (open only). Runs via /bin/sh -c. Defaults to $SHELL."
                },
                "rows": { "type": "integer", "minimum": 8, "maximum": 200 },
                "cols": { "type": "integer", "minimum": 20, "maximum": 400 },
                "input": {
                    "type": "array",
                    "description": "Key chunks: [{\"text\":\"...\"} or {\"key\":\"enter|tab|esc|backspace|up|down|left|right|home|end|pageup|pagedown|ctrl-c|ctrl-d|ctrl-z|ctrl-l|ctrl-u\"}]",
                    "items": { "type": "object" }
                },
                "signal": {
                    "type": "string",
                    "enum": ["int", "term", "hup", "kill"]
                }
            }
        }),
    )
}

fn fence_for(text: &str) -> String {
    let max_run = text
        .chars()
        .fold((0usize, 0usize), |(max, cur), ch| {
            if ch == '`' {
                (max.max(cur + 1), cur + 1)
            } else {
                (max, 0)
            }
        })
        .0;
    "`".repeat(max_run.max(2) + 1)
}

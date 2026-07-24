use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolContext, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_bus::EventBus;
use goat_daemon::Manager;
use goat_protocol::Event as CodeEvent;
use goat_types::{CodeUpdateKind, ConversationId, Event, ProfileId};
use serde::Deserialize;
use serde_json::json;

pub const CODE_TASK: ToolName = ToolName::from_static("code_task");

pub fn register(registry: &mut ToolRegistry, bus: EventBus, manager: Manager) {
    registry.insert_handler(spec(), Arc::new(CodeTool { bus, manager }), true);
}

struct CodeTool {
    bus: EventBus,
    manager: Manager,
}

#[derive(Debug, Deserialize)]
struct CodeArgs {
    cwd: String,
    prompt: String,
}

#[async_trait]
impl ToolHandler for CodeTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args: CodeArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid code_task input: {e}")),
        };
        let cwd = PathBuf::from(&args.cwd);
        if !cwd.is_dir() {
            return ToolOutput::error(format!("cwd is not a directory: {}", args.cwd));
        }

        let events = match self.manager.delegate_code(cwd, args.prompt).await {
            Ok(events) => events,
            Err(e) => return ToolOutput::error(format!("could not start coding task: {e}")),
        };

        let bus = self.bus.clone();
        let persona = ctx.persona;
        let conversation = ctx.conversation.clone();
        tokio::spawn(pump(events, bus, persona, conversation));

        ToolOutput::structured(json!({
            "delegated": true,
            "note": "coding task started; progress will stream to this channel"
        }))
    }
}

async fn pump(
    mut events: tokio::sync::mpsc::Receiver<CodeEvent>,
    bus: EventBus,
    persona: ProfileId,
    conversation: ConversationId,
) {
    let publish = |kind: CodeUpdateKind, text: String| {
        bus.publish(Event::CodeUpdate {
            profile: persona,
            conversation: conversation.clone(),
            kind,
            text,
        });
    };

    while let Some(event) = events.recv().await {
        match event {
            CodeEvent::ToolStarted { call, .. } => {
                publish(CodeUpdateKind::Progress, format!("running {}", call.name));
            }
            CodeEvent::TextDone { text, .. } if !text.trim().is_empty() => {
                publish(CodeUpdateKind::Progress, text);
            }
            CodeEvent::AskStarted { questions, .. } => {
                let rendered = questions
                    .iter()
                    .map(|q| q.question.clone())
                    .collect::<Vec<_>>()
                    .join("; ");
                publish(CodeUpdateKind::Ask, rendered);
            }
            CodeEvent::TaskDone { interrupted, .. } => {
                let text = if interrupted {
                    "coding task interrupted".to_string()
                } else {
                    "coding task done".to_string()
                };
                publish(CodeUpdateKind::Done, text);
                break;
            }
            CodeEvent::Error { message, .. } => {
                publish(
                    CodeUpdateKind::Failed,
                    format!("coding task failed: {message}"),
                );
                break;
            }
            _ => {}
        }
    }
}

fn spec() -> ToolSpec {
    ToolSpec::new(
        CODE_TASK,
        "Delegate a coding task to the code engine in a project directory. Runs \
         asynchronously in the same daemon; progress, questions, and the result \
         stream back to this conversation. Use for multi-step code work (edits, \
         refactors, PRs) rather than doing it yourself.",
        json!({
            "type": "object",
            "required": ["cwd", "prompt"],
            "properties": {
                "cwd": { "type": "string", "description": "Absolute path to the project directory." },
                "prompt": { "type": "string", "description": "What the code engine should do." }
            }
        }),
    )
}

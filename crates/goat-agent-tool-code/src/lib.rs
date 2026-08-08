use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolCaller, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_daemon::CodeSessionHub;
use serde::Deserialize;
use serde_json::json;

pub const CODE_TASK: ToolName = ToolName::from_static("code_task");

pub fn register(registry: &mut ToolRegistry, manager: CodeSessionHub) {
    registry.insert_handler(spec(), Arc::new(CodeTool { manager }), true);
}

struct CodeTool {
    manager: CodeSessionHub,
}

#[derive(Debug, Deserialize)]
struct CodeArgs {
    cwd: String,
    prompt: String,
}

#[async_trait]
impl ToolHandler for CodeTool {
    async fn call(&self, _ctx: ToolCaller, call: ToolCall) -> ToolOutput {
        let args: CodeArgs = match serde_json::from_value(call.arguments) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("invalid code_task input: {e}")),
        };
        let cwd = PathBuf::from(&args.cwd);
        if !cwd.is_dir() {
            return ToolOutput::error(format!("cwd is not a directory: {}", args.cwd));
        }

        if let Err(e) = self.manager.delegate_code(cwd, args.prompt).await {
            return ToolOutput::error(format!("could not start coding task: {e}"));
        }

        ToolOutput::structured(json!({
            "delegated": true,
            "note": "coding task started; it runs asynchronously in the background and does not stream progress back"
        }))
    }
}

fn spec() -> ToolSpec {
    ToolSpec::new(
        CODE_TASK,
        "Delegate a coding task to the code engine in a project directory. Runs \
         asynchronously in the background in the same daemon and does not stream \
         progress back. Use for multi-step code work (edits, refactors, PRs) \
         rather than doing it yourself.",
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

use std::borrow::Cow;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use goat_tool::{Tool, ToolCall, ToolContext, ToolFuture, ToolName, ToolOutput, ToolRegistry};
use serde::Deserialize;
use serde_json::json;

pub const CODE_TASK: ToolName = ToolName::from_static("code_task");

pub type DelegateFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

pub trait CodeDelegate: Send + Sync + 'static {
    fn delegate_code(&self, cwd: PathBuf, prompt: String) -> DelegateFuture<'_>;
}

pub fn register(registry: &mut ToolRegistry, delegate: impl CodeDelegate) {
    registry.insert(Arc::new(CodeTool {
        delegate: Arc::new(delegate),
    }));
}

struct CodeTool {
    delegate: Arc<dyn CodeDelegate>,
}

#[derive(Debug, Deserialize)]
struct CodeArgs {
    cwd: String,
    prompt: String,
}

impl Tool for CodeTool {
    fn name(&self) -> ToolName {
        CODE_TASK.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        Cow::Borrowed(
            "Delegate a coding task to the code engine in a project directory. Runs \
             asynchronously in the background in the same daemon and does not stream \
             progress back. Use for multi-step code work (edits, refactors, PRs) \
             rather than doing it yourself.",
        )
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["cwd", "prompt"],
            "properties": {
                "cwd": { "type": "string", "description": "Absolute path to the project directory." },
                "prompt": { "type": "string", "description": "What the code engine should do." }
            }
        })
    }

    fn call<'a>(&'a self, call: &'a ToolCall, _ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: CodeArgs = match serde_json::from_value(call.arguments.clone()) {
                Ok(a) => a,
                Err(e) => {
                    return Ok(ToolOutput::error(format!("invalid code_task input: {e}")));
                }
            };
            let cwd = PathBuf::from(&args.cwd);
            if !cwd.is_dir() {
                return Ok(ToolOutput::error(format!(
                    "cwd is not a directory: {}",
                    args.cwd
                )));
            }

            if let Err(e) = self.delegate.delegate_code(cwd, args.prompt).await {
                return Ok(ToolOutput::error(format!(
                    "could not start coding task: {e}"
                )));
            }

            Ok(ToolOutput::structured(json!({
                "delegated": true,
                "note": "coding task started; it runs asynchronously in the background and does not stream progress back"
            })))
        })
    }
}

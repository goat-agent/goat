use std::sync::Arc;

use async_trait::async_trait;
use goat_tool::{ToolCall, ToolContext, ToolFactory, ToolHandler, ToolName, ToolOutput, ToolSpec};
use serde_json::json;

pub const NAME: ToolName = ToolName::from_static("echo.echo");

pub struct EchoTool;

#[async_trait]
impl ToolHandler for EchoTool {
    async fn call(&self, _ctx: ToolContext, call: ToolCall) -> ToolOutput {
        ToolOutput::structured(json!({ "echo": call.arguments }))
    }
}

fn spec() -> ToolSpec {
    ToolSpec::new(
        NAME,
        "Echo the provided JSON arguments. Intended for deterministic tool-loop smoke tests.",
        json!({
            "type": "object",
            "additionalProperties": true
        }),
    )
}

fn ctor() -> Arc<dyn ToolHandler> {
    Arc::new(EchoTool)
}

inventory::submit! {
    ToolFactory { name: NAME, default_enabled: false, spec, ctor }
}

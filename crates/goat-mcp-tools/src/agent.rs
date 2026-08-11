use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolCaller, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};

use crate::{McpToolSource, ResolvedTool};

pub fn install(registry: &mut ToolRegistry, tools: Vec<ResolvedTool>) -> Vec<ToolName> {
    let mut names = Vec::new();
    for tool in tools {
        let Ok(name) = ToolName::new(tool.exposed_name.clone()) else {
            continue;
        };
        registry.insert_handler(
            ToolSpec::new(name.clone(), tool.description, tool.input_schema),
            Arc::new(Passthrough {
                source: tool.source,
                tool: tool.original_name,
            }),
            tool.enabled,
        );
        names.push(name);
    }
    names
}

struct Passthrough {
    source: Arc<dyn McpToolSource>,
    tool: String,
}

#[async_trait]
impl ToolHandler for Passthrough {
    async fn call(&self, ctx: ToolCaller, call: ToolCall) -> ToolOutput {
        match self
            .source
            .call(&self.tool, call.arguments, Some(ctx.agent))
            .await
        {
            Ok(outcome) => match outcome.structured {
                Some(value) => ToolOutput::structured(value),
                None => ToolOutput::text(outcome.text),
            },
            Err(message) => ToolOutput::error(message),
        }
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolCaller, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_api::Holder;
use goat_daemon::{CodeSessionHub, ComputerRelay};
use goat_tool_computer::Computer;
use parking_lot::Mutex;

pub const COMPUTER: ToolName = ToolName::from_static("computer");

pub fn register(registry: &mut ToolRegistry, manager: CodeSessionHub) {
    registry.insert_handler(
        ToolSpec::new(
            COMPUTER,
            goat_tool_computer::DESCRIPTION,
            goat_tool_computer::parameters(),
        ),
        Arc::new(AgentComputer {
            manager,
            computers: Mutex::new(HashMap::new()),
        }),
        true,
    );
}

struct AgentComputer {
    manager: CodeSessionHub,
    computers: Mutex<HashMap<Holder, Arc<Computer>>>,
}

impl AgentComputer {
    fn computer(&self, holder: &Holder) -> Arc<Computer> {
        self.computers
            .lock()
            .entry(holder.clone())
            .or_insert_with(|| {
                Arc::new(Computer::new(Arc::new(ComputerRelay::new(
                    self.manager.broker(),
                    holder.clone(),
                ))))
            })
            .clone()
    }
}

#[async_trait]
impl ToolHandler for AgentComputer {
    async fn call(&self, ctx: ToolCaller, call: ToolCall) -> ToolOutput {
        let computer = self.computer(&Holder::agent(&ctx.agent_slug));
        match computer.run(&call.arguments.to_string()).await {
            Ok(output) => deliver(output),
            Err(err) => ToolOutput::error(err.to_string()),
        }
    }
}

fn deliver(output: goat_tool::ToolOutput) -> ToolOutput {
    match output.content {
        goat_tool::ToolContent::Text(text) => ToolOutput::text(text),
        goat_tool::ToolContent::Image(image) => ToolOutput::image(
            output.summary.unwrap_or_default(),
            image.media_type,
            image.data,
        ),
    }
}

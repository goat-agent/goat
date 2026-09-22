use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use goat_api::Holder;
use goat_tool::{Tool, ToolCall, ToolContext, ToolFuture, ToolName, ToolRegistry};
use parking_lot::Mutex;

use crate::{Computer, Transport};

pub const COMPUTER: ToolName = ToolName::from_static("computer");

pub trait AgentTransport: Send + Sync + 'static {
    fn transport(&self, holder: &Holder) -> Arc<dyn Transport>;
}

pub fn register(registry: &mut ToolRegistry, host: impl AgentTransport) {
    registry.insert(Arc::new(AgentComputer {
        host: Arc::new(host),
        computers: Mutex::new(HashMap::new()),
    }));
}

struct AgentComputer {
    host: Arc<dyn AgentTransport>,
    computers: Mutex<HashMap<Holder, Arc<Computer>>>,
}

impl AgentComputer {
    fn computer(&self, holder: &Holder) -> Arc<Computer> {
        self.computers
            .lock()
            .entry(holder.clone())
            .or_insert_with(|| Arc::new(Computer::new(self.host.transport(holder))))
            .clone()
    }
}

impl Tool for AgentComputer {
    fn name(&self) -> ToolName {
        COMPUTER.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        Cow::Borrowed(crate::DESCRIPTION)
    }

    fn parameters(&self) -> serde_json::Value {
        crate::parameters()
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let agent = ctx.agent_context()?;
            let computer = self.computer(&Holder::agent(&agent.slug));
            computer.run(&call.arguments.to_string()).await
        })
    }
}

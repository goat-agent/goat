use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use goat_api::Holder;
use goat_tool::{Tool, ToolCall, ToolContext, ToolError, ToolFuture, ToolName, ToolRegistry};

use crate::{Browser, Transport};

pub const BROWSER: ToolName = ToolName::from_static("browser");

const MAX_OUTPUT_BYTES: usize = 12_000;

pub trait AgentTransport: Send + Sync + 'static {
    fn transport(&self, holder: &Holder) -> Arc<dyn Transport>;
}

pub fn register(registry: &mut ToolRegistry, host: impl AgentTransport) {
    registry.insert(Arc::new(AgentBrowser {
        host: Arc::new(host),
        browsers: Mutex::new(HashMap::new()),
    }));
}

struct AgentBrowser {
    host: Arc<dyn AgentTransport>,
    browsers: Mutex<HashMap<Holder, Arc<Browser>>>,
}

impl AgentBrowser {
    fn browser(&self, holder: &Holder) -> Arc<Browser> {
        let mut browsers = self
            .browsers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        browsers
            .entry(holder.clone())
            .or_insert_with(|| Arc::new(Browser::new(self.host.transport(holder))))
            .clone()
    }
}

impl Tool for AgentBrowser {
    fn name(&self) -> ToolName {
        BROWSER.clone()
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
            let browser = self.browser(&Holder::agent(&agent.slug));
            browser
                .run(&call.arguments.to_string(), MAX_OUTPUT_BYTES)
                .await
                .map_err(|e| ToolError::execution(e.to_string()))
        })
    }
}

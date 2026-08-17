use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolCaller, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_api::Holder;
use goat_daemon::{BrowserRelay, CodeSessionHub};
use goat_tool_browser::Browser;

pub const BROWSER: ToolName = ToolName::from_static("browser");

const MAX_OUTPUT_BYTES: usize = 12_000;

pub fn register(registry: &mut ToolRegistry, manager: CodeSessionHub) {
    registry.insert_handler(
        spec(),
        Arc::new(AgentBrowser {
            manager,
            browsers: Mutex::new(HashMap::new()),
        }),
        true,
    );
}

struct AgentBrowser {
    manager: CodeSessionHub,
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
            .or_insert_with(|| {
                Arc::new(Browser::new(Arc::new(BrowserRelay::new(
                    self.manager.broker(),
                    self.manager.browser_events(),
                    holder.clone(),
                ))))
            })
            .clone()
    }
}

#[async_trait]
impl ToolHandler for AgentBrowser {
    async fn call(&self, ctx: ToolCaller, call: ToolCall) -> ToolOutput {
        let browser = self.browser(&Holder::agent(&ctx.agent_slug));
        match browser
            .run(&call.arguments.to_string(), MAX_OUTPUT_BYTES)
            .await
        {
            Ok(output) => deliver(output),
            Err(err) => ToolOutput::error(err.to_string()),
        }
    }
}

fn deliver(output: goat_tool::ToolOutput) -> ToolOutput {
    match output.content {
        goat_tool::ToolContent::Text(text) => ToolOutput::text(text),
        goat_tool::ToolContent::Image(_) => ToolOutput::error(
            "an agent turn carries no images, so a screenshot cannot be delivered; read the page \
             with read_content, read_viewport, find_text, or inspect instead",
        ),
    }
}

fn spec() -> ToolSpec {
    ToolSpec::new(
        BROWSER,
        goat_tool_browser::DESCRIPTION,
        goat_tool_browser::parameters(),
    )
}

#[cfg(test)]
mod tests {
    use super::deliver;

    #[test]
    fn a_page_read_reaches_the_agent_as_text() {
        let delivered = deliver(goat_tool::ToolOutput::text("heading: Inbox"));
        assert!(!delivered.is_error);
        assert_eq!(delivered.text_for_model(), "heading: Inbox");
    }

    #[test]
    fn a_screenshot_is_refused_with_the_readable_alternatives() {
        let delivered = deliver(goat_tool::ToolOutput::png("iVBOR"));
        assert!(delivered.is_error);
        assert!(delivered.text_for_model().contains("read_content"));
    }
}

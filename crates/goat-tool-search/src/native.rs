use std::borrow::Cow;
use std::{future::Future, pin::Pin, sync::Arc};

use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolCall, ToolContext, ToolError, ToolFuture, ToolName, ToolOutput};

pub struct NativeSearchRequest {
    pub query: String,
}

pub type NativeSearchFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

pub trait NativeSearchService: Send + Sync {
    fn search<'a>(
        &'a self,
        request: NativeSearchRequest,
        ctx: ToolContext<'a>,
    ) -> NativeSearchFuture<'a>;
}

pub struct NativeWebSearchTool {
    service: Arc<dyn NativeSearchService>,
}

impl NativeWebSearchTool {
    pub fn new(service: Arc<dyn NativeSearchService>) -> Self {
        Self { service }
    }
}

#[derive(serde::Deserialize)]
struct Input {
    query: String,
}

impl Tool for NativeWebSearchTool {
    fn name(&self) -> ToolName {
        ToolName::from_static("WebSearch")
    }

    fn description(&self) -> Cow<'static, str> {
        "Search the web and return a list of result titles and URLs. Use it to find current information, documentation, or sources; then read the most relevant pages with WebFetch.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        })
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: Input =
                serde_json::from_value(call.arguments.clone()).map_err(ToolError::from)?;
            if input.query.trim().is_empty() {
                return Err(ToolError::invalid_input("query must not be empty"));
            }
            let content = self
                .service
                .search(NativeSearchRequest { query: input.query }, ctx)
                .await
                .map_err(ToolError::execution)?;
            Ok(ToolOutput::text(content))
        })
    }

    fn handles_cancellation(&self) -> bool {
        true
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => ToolDisplay::primary(goat_tool::display::call_sig(
                self.name().as_str(),
                &[args.query.as_str()],
            )),
            Err(_) => goat_tool::display::generic_named(self.name().as_str(), input),
        }
    }
}

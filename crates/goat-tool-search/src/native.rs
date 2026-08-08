use std::{future::Future, pin::Pin, sync::Arc};

use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolInvocation, ToolOutput};

pub struct NativeSearchRequest {
    pub query: String,
}

pub type NativeSearchFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

pub trait NativeSearchService: Send + Sync {
    fn search<'a>(
        &'a self,
        request: NativeSearchRequest,
        invocation: ToolInvocation<'a>,
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
    fn name(&self) -> &'static str {
        "WebSearch"
    }

    fn description(&self) -> &'static str {
        "Search the web and return a list of result titles and URLs. Use it to find current information, documentation, or sources; then read the most relevant pages with WebFetch."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        })
    }

    fn run<'a>(&'a self, _input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async {
            Err(ToolError::execution(
                "native search invocation is unavailable",
            ))
        })
    }

    fn invoke<'a>(
        &'a self,
        input: &'a str,
        _ctx: &'a ToolContext,
        invocation: ToolInvocation<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: Input = serde_json::from_str(input).map_err(ToolError::from)?;
            if input.query.trim().is_empty() {
                return Err(ToolError::invalid_input("query must not be empty"));
            }
            let content = self
                .service
                .search(NativeSearchRequest { query: input.query }, invocation)
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
                self.name(),
                &[args.query.as_str()],
            )),
            Err(_) => goat_tool::display::generic_named(self.name(), input),
        }
    }
}

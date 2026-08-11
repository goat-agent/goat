mod agent;
mod code;
mod manager;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use goat_mcp::{McpImage, McpToolResult};
use goat_types::AgentId;
use serde_json::Value;

pub use agent::install;
pub use code::adapt;
pub use manager::{exposed_tool_name, from_manager};

pub type McpCallFuture<'a> = Pin<Box<dyn Future<Output = Result<McpOutcome, String>> + Send + 'a>>;

#[derive(Clone, Debug, Default)]
pub struct McpOutcome {
    pub text: String,
    pub structured: Option<Value>,
    pub image: Option<McpImage>,
}

impl McpOutcome {
    #[must_use]
    pub fn structured(value: Value) -> Self {
        Self {
            text: match &value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            },
            structured: Some(value),
            image: None,
        }
    }

    pub fn from_result(tool: &str, result: McpToolResult) -> Result<Self, String> {
        let text = result.text();
        if result.is_error {
            let detail = if text.is_empty() {
                "the server reported an error".to_owned()
            } else {
                text
            };
            return Err(format!("mcp tool {tool} returned an error: {detail}"));
        }
        let image = result.first_image().cloned();
        Ok(Self {
            text,
            structured: result.structured,
            image,
        })
    }
}

pub trait McpToolSource: Send + Sync + 'static {
    fn label(&self) -> &str;

    fn call<'a>(
        &'a self,
        tool: &'a str,
        arguments: Value,
        caller: Option<AgentId>,
    ) -> McpCallFuture<'a>;
}

pub struct ResolvedTool {
    pub exposed_name: String,
    pub original_name: String,
    pub description: String,
    pub input_schema: Value,
    pub enabled: bool,
    pub source: Arc<dyn McpToolSource>,
}

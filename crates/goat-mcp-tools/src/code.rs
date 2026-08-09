use std::collections::HashSet;
use std::sync::Arc;

use goat_protocol::ToolDisplay;
use goat_tool::{
    Tool, ToolDefinitionContext, ToolError, ToolFuture, ToolImage, ToolOutput, ToolSandbox,
};
use serde_json::Value;

use crate::{McpOutcome, McpToolSource, ResolvedTool};

pub fn adapt(tools: Vec<ResolvedTool>) -> Vec<Box<dyn Tool>> {
    let mut taken = HashSet::new();
    let mut adapters: Vec<Adapter> = tools
        .into_iter()
        .filter(|tool| taken.insert(tool.exposed_name.clone()))
        .map(Adapter::new)
        .collect();
    adapters.sort_by(|a, b| a.name.cmp(b.name));
    adapters
        .into_iter()
        .map(|tool| Box::new(tool) as Box<dyn Tool>)
        .collect()
}

struct Adapter {
    name: &'static str,
    description: &'static str,
    parameters: Value,
    original_name: String,
    label: String,
    enabled: bool,
    source: Arc<dyn McpToolSource>,
}

impl Adapter {
    fn new(tool: ResolvedTool) -> Self {
        Self {
            name: leak(tool.exposed_name),
            description: leak(tool.description),
            parameters: tool.input_schema,
            original_name: tool.original_name,
            label: tool.source.label().to_owned(),
            enabled: tool.enabled,
            source: tool.source,
        }
    }
}

impl Tool for Adapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn enabled(&self, _context: ToolDefinitionContext) -> bool {
        self.enabled
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let arguments = arguments_from(input)?;
            let outcome = self
                .source
                .call(&self.original_name, arguments, None)
                .await
                .map_err(ToolError::execution)?;
            Ok(output_from(outcome))
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        ToolDisplay::with_detail(
            format!("{} on {}", self.original_name, self.label),
            input.to_owned(),
        )
    }
}

fn arguments_from(input: &str) -> Result<Value, ToolError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(trimmed)?)
}

fn output_from(outcome: McpOutcome) -> ToolOutput {
    let mut parts = Vec::new();
    if !outcome.text.is_empty() {
        parts.push(outcome.text);
    }
    if let Some(structured) = &outcome.structured {
        parts.push(format!("structuredContent: {structured}"));
    }
    if !parts.is_empty() {
        let summary = summary(&parts);
        return ToolOutput::text(parts.join("\n")).with_summary(summary);
    }
    if let Some(image) = outcome.image {
        return ToolOutput::image(ToolImage {
            media_type: image.media_type,
            data: image.data,
        })
        .with_summary("image");
    }
    ToolOutput::text(String::new())
}

fn summary(parts: &[String]) -> String {
    parts
        .iter()
        .find_map(|part| part.lines().find(|line| !line.trim().is_empty()))
        .map_or_else(String::new, |line| line.chars().take(80).collect())
}

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_mcp::{McpContent, McpToolResult};
    use serde_json::json;

    fn outcome(content: Vec<McpContent>, structured: Option<Value>, is_error: bool) -> McpOutcome {
        McpOutcome::from_result(
            "tool",
            McpToolResult {
                content,
                structured,
                is_error,
            },
        )
        .unwrap()
    }

    #[test]
    fn text_and_structured_are_joined() {
        let out = output_from(outcome(
            vec![McpContent::Text("{\"ok\":true}".to_owned())],
            Some(json!({"ok": true})),
            false,
        ));
        assert_eq!(
            out.as_text().unwrap(),
            "{\"ok\":true}\nstructuredContent: {\"ok\":true}"
        );
    }

    #[test]
    fn an_error_result_names_the_tool() {
        let err = McpOutcome::from_result(
            "tool",
            McpToolResult {
                content: vec![McpContent::Text("bad".to_owned())],
                structured: None,
                is_error: true,
            },
        )
        .unwrap_err();
        assert!(err.contains("tool"));
        assert!(err.contains("bad"));
    }

    #[test]
    fn an_error_with_no_detail_still_names_the_tool() {
        let err = McpOutcome::from_result(
            "tool",
            McpToolResult {
                is_error: true,
                ..McpToolResult::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("tool"));
    }

    #[test]
    fn an_image_only_result_becomes_an_image_output() {
        let out = output_from(outcome(
            vec![McpContent::Image(goat_mcp::McpImage {
                media_type: "image/png".to_owned(),
                data: "YmFzZTY0".to_owned(),
            })],
            None,
            false,
        ));
        assert!(out.as_text().is_none());
    }

    #[test]
    fn blank_input_means_no_arguments() {
        assert_eq!(arguments_from("   ").unwrap(), Value::Null);
        assert_eq!(arguments_from("{\"a\":1}").unwrap(), json!({"a": 1}));
        assert!(arguments_from("not json").is_err());
    }

    #[test]
    fn a_structured_outcome_renders_a_string_body_verbatim() {
        let out = McpOutcome::structured(json!("plain"));
        assert_eq!(out.text, "plain");
    }
}

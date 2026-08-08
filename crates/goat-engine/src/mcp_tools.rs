use std::collections::HashSet;
use std::sync::Arc;

use goat_mcp::{McpManager, McpSession, McpTool, McpToolResult, sanitize_component};
use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolError, ToolFuture, ToolImage, ToolOutput, ToolSandbox};
use serde_json::Value;

pub fn adapt(manager: &McpManager) -> Vec<Box<dyn Tool>> {
    let mut used = HashSet::new();
    let mut adapters = Vec::new();
    for server in manager.servers() {
        for tool in &server.tools {
            let exposed = unique_tool_name(&mut used, &server.name, &tool.name);
            adapters.push(McpToolAdapter::new(
                exposed,
                server.name.clone(),
                tool.clone(),
                server.session.clone(),
            ));
        }
    }
    adapters.sort_by(|a, b| a.name.cmp(b.name));
    adapters
        .into_iter()
        .map(|tool| Box::new(tool) as Box<dyn Tool>)
        .collect()
}

pub fn exposed_tool_name(server: &str, tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_component(server),
        sanitize_component(tool)
    )
}

fn unique_tool_name(used: &mut HashSet<String>, server: &str, tool: &str) -> String {
    let base = exposed_tool_name(server, tool);
    if used.insert(base.clone()) {
        return base;
    }
    let mut index = 2;
    loop {
        let candidate = format!("{base}_{index}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

#[derive(Clone)]
struct McpToolAdapter {
    name: &'static str,
    description: &'static str,
    parameters: Value,
    original_name: String,
    server_name: String,
    session: Arc<McpSession>,
}

impl McpToolAdapter {
    fn new(
        exposed_name: String,
        server_name: String,
        tool: McpTool,
        session: Arc<McpSession>,
    ) -> Self {
        let description = tool
            .description
            .map_or_else(String::new, std::borrow::Cow::into_owned);
        Self {
            name: leak(exposed_name),
            description: leak(description),
            parameters: Value::Object((*tool.input_schema).clone()),
            original_name: tool.name.into_owned(),
            server_name,
            session,
        }
    }
}

impl Tool for McpToolAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let arguments = arguments_from(input)?;
            let result = self
                .session
                .call(&self.original_name, arguments)
                .await
                .map_err(|err| ToolError::execution(err.to_string()))?;
            output_from(&self.original_name, &result)
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        ToolDisplay::with_detail(
            format!("{} on {}", self.original_name, self.server_name),
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

fn output_from(tool_name: &str, result: &McpToolResult) -> Result<ToolOutput, ToolError> {
    let mut parts = Vec::new();
    let text = result.text();
    if !text.is_empty() {
        parts.push(text);
    }
    if let Some(structured) = &result.structured {
        parts.push(format!("structuredContent: {structured}"));
    }
    if result.is_error {
        let message = if parts.is_empty() {
            "MCP tool returned an error".to_owned()
        } else {
            parts.join("\n")
        };
        return Err(ToolError::execution(format!(
            "mcp tool {tool_name} returned an error: {message}"
        )));
    }
    if !parts.is_empty() {
        let joined = parts.join("\n");
        let summary = summary(&parts);
        return Ok(ToolOutput::text(joined).with_summary(summary));
    }
    if let Some(image) = result.first_image() {
        return Ok(ToolOutput::image(ToolImage {
            media_type: image.media_type.clone(),
            data: image.data.clone(),
        })
        .with_summary("image"));
    }
    Ok(ToolOutput::text(String::new()))
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
    use goat_mcp::{McpContent, McpImage};
    use serde_json::json;

    fn result(
        content: Vec<McpContent>,
        structured: Option<Value>,
        is_error: bool,
    ) -> McpToolResult {
        McpToolResult {
            content,
            structured,
            is_error,
        }
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(
            exposed_tool_name("File System", "Read.Path"),
            "mcp__file_system__read_path"
        );
        assert_eq!(exposed_tool_name("한글", "!!!"), "mcp__unnamed__unnamed");
    }

    #[test]
    fn unique_names_are_deterministic() {
        let mut used = HashSet::new();
        assert_eq!(unique_tool_name(&mut used, "a-b", "c"), "mcp__a_b__c");
        assert_eq!(unique_tool_name(&mut used, "a_b", "c"), "mcp__a_b__c_2");
    }

    #[test]
    fn converts_text_and_structured_result() {
        let out = output_from(
            "tool",
            &result(
                vec![McpContent::Text("{\"ok\":true}".to_owned())],
                Some(json!({"ok": true})),
                false,
            ),
        )
        .unwrap();
        assert_eq!(
            out.as_text().unwrap(),
            "{\"ok\":true}\nstructuredContent: {\"ok\":true}"
        );
    }

    #[test]
    fn converts_error_result_to_error() {
        let out = output_from(
            "tool",
            &result(vec![McpContent::Text("bad".to_owned())], None, true),
        );
        assert!(matches!(out, Err(err) if err.to_string().contains("bad")));
    }

    #[test]
    fn an_error_with_no_detail_still_names_the_tool() {
        let out = output_from("tool", &result(Vec::new(), None, true));
        assert!(matches!(out, Err(err) if err.to_string().contains("tool")));
    }

    #[test]
    fn an_image_only_result_becomes_an_image_output() {
        let out = output_from(
            "tool",
            &result(
                vec![McpContent::Image(McpImage {
                    media_type: "image/png".to_owned(),
                    data: "YmFzZTY0".to_owned(),
                })],
                None,
                false,
            ),
        )
        .unwrap();
        assert!(out.as_text().is_none());
    }

    #[test]
    fn blank_input_means_no_arguments() {
        assert_eq!(arguments_from("   ").unwrap(), Value::Null);
        assert_eq!(arguments_from("{\"a\":1}").unwrap(), json!({"a": 1}));
        assert!(arguments_from("not json").is_err());
    }
}

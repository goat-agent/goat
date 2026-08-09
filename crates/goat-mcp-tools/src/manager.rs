use std::collections::HashSet;
use std::sync::Arc;

use goat_mcp::{McpManager, McpSession, sanitize_component};
use goat_types::AgentId;
use serde_json::Value;

use crate::{McpCallFuture, McpOutcome, McpToolSource, ResolvedTool};

pub fn from_manager(manager: &McpManager) -> Vec<ResolvedTool> {
    let mut used = HashSet::new();
    let mut tools = Vec::new();
    for server in manager.servers() {
        let source: Arc<dyn McpToolSource> = Arc::new(ServerSource {
            label: server.name.clone(),
            session: server.session.clone(),
        });
        for tool in &server.tools {
            let exposed = unique_tool_name(&mut used, &server.name, &tool.name);
            tools.push(ResolvedTool {
                exposed_name: exposed,
                original_name: tool.name.to_string(),
                description: tool.description.as_deref().unwrap_or_default().to_owned(),
                input_schema: Value::Object((*tool.input_schema).clone()),
                enabled: true,
                source: source.clone(),
            });
        }
    }
    tools
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

struct ServerSource {
    label: String,
    session: Arc<McpSession>,
}

impl McpToolSource for ServerSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn call<'a>(
        &'a self,
        tool: &'a str,
        arguments: Value,
        _caller: Option<AgentId>,
    ) -> McpCallFuture<'a> {
        Box::pin(async move {
            let result = self
                .session
                .call(tool, arguments)
                .await
                .map_err(|err| err.to_string())?;
            McpOutcome::from_result(tool, result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

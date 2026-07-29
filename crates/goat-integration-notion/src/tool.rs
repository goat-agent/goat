use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolContext, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_auth::CredentialStore;
use goat_integration::{BindingMap, IntegrationBinding, IntegrationRuntime, drop_placeholder_args};
use goat_types::ProfileId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use crate::{ID, mcp};

const TOOLS_STREAM: &str = "tools";

pub fn tool_name(raw: &str) -> String {
    format!("notion_{}", mcp::normalize(raw))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl From<rmcp::model::Tool> for CachedTool {
    fn from(tool: rmcp::model::Tool) -> Self {
        Self {
            name: tool.name.to_string(),
            description: tool.description.as_deref().unwrap_or_default().to_string(),
            input_schema: Value::Object(tool.input_schema.as_ref().clone()),
        }
    }
}

pub async fn register(
    registry: &mut ToolRegistry,
    runtime: &IntegrationRuntime,
    bindings: Arc<BindingMap>,
) -> Vec<ToolName> {
    let Some((persona, binding)) = first_binding(&bindings) else {
        return Vec::new();
    };
    let tools = discover(runtime, persona, binding).await;
    let mut names = Vec::new();
    for tool in tools {
        let Ok(name) = ToolName::new(tool_name(&tool.name)) else {
            warn!(tool = %tool.name, "skipping notion mcp tool with unusable name");
            continue;
        };
        registry.insert_handler(
            ToolSpec::new(name.clone(), tool.description, tool.input_schema),
            Arc::new(McpPassthrough {
                credentials: runtime.credentials.clone(),
                bindings: bindings.clone(),
                tool: tool.name,
            }),
            true,
        );
        names.push(name);
    }
    names
}

fn first_binding(bindings: &BindingMap) -> Option<(ProfileId, &IntegrationBinding)> {
    bindings
        .iter()
        .min_by_key(|(persona, _)| persona.to_string())
        .map(|(persona, binding)| (*persona, binding))
}

async fn discover(
    runtime: &IntegrationRuntime,
    persona: ProfileId,
    binding: &IntegrationBinding,
) -> Vec<CachedTool> {
    match fetch_live(&runtime.credentials, binding).await {
        Ok(tools) => {
            if let Ok(raw) = serde_json::to_string(&tools)
                && let Err(e) = runtime
                    .save_state(persona, &ID, &binding.account, TOOLS_STREAM, &raw)
                    .await
            {
                warn!(error = %e, "failed to cache notion tool list");
            }
            tools
        }
        Err(e) => {
            warn!(error = %e, "notion tool discovery failed; using cached list");
            match runtime
                .load_state(persona, &ID, &binding.account, TOOLS_STREAM)
                .await
            {
                Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
                _ => Vec::new(),
            }
        }
    }
}

async fn fetch_live(
    credentials: &CredentialStore,
    binding: &IntegrationBinding,
) -> goat_integration::IntegrationResult<Vec<CachedTool>> {
    let client_id = binding.config.get("client_id").and_then(Value::as_str);
    let auth = mcp::resolve_auth(credentials, &binding.account, client_id)?;
    let session = mcp::connect(&auth).await?;
    let tools = session.list_tools().await;
    mcp::persist_tokens(credentials, &binding.account, &session).await;
    session.close().await;
    Ok(tools?.into_iter().map(CachedTool::from).collect())
}

struct McpPassthrough {
    credentials: CredentialStore,
    bindings: Arc<BindingMap>,
    tool: String,
}

#[async_trait]
impl ToolHandler for McpPassthrough {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let Some(binding) = self.bindings.get(&ctx.persona) else {
            return ToolOutput::error(
                "notion is not configured for this agent; run `goat agent integration add notion`",
            );
        };
        let client_id = binding.config.get("client_id").and_then(Value::as_str);
        let auth = match mcp::resolve_auth(&self.credentials, &binding.account, client_id) {
            Ok(auth) => auth,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        let session = match mcp::connect(&auth).await {
            Ok(session) => session,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        let arguments = drop_placeholder_args(call.arguments);
        debug!(tool = %self.tool, arguments = %arguments, "calling notion mcp tool");
        let result = session.call(&self.tool, arguments).await;
        mcp::persist_tokens(&self.credentials, &binding.account, &session).await;
        session.close().await;
        match result {
            Ok(data) => ToolOutput::structured(data),
            Err(e) => ToolOutput::error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_name_collapses_every_vendor_naming_style() {
        assert_eq!(tool_name("notion-search"), "notion_search");
        assert_eq!(tool_name("search"), "notion_search");
        assert_eq!(tool_name("notion_search"), "notion_search");
        assert_eq!(tool_name("notion-create-pages"), "notion_create_pages");
        assert_eq!(
            tool_name("notion-query-data-sources"),
            "notion_query_data_sources"
        );
    }

    #[test]
    fn registered_names_are_valid_tool_names() {
        for raw in [
            "notion-search",
            "notion-fetch",
            "notion-create-pages",
            "notion-query-data-sources",
            "notion-get-async-task",
        ] {
            assert!(
                ToolName::new(tool_name(raw)).is_ok(),
                "`{raw}` produced an unusable tool name",
            );
        }
        assert!(ToolName::new(tool_name("bad.name")).is_err());
    }

    #[test]
    fn cached_tools_round_trip() {
        let cached = vec![CachedTool {
            name: "notion-search".into(),
            description: "Search Notion".into(),
            input_schema: json!({ "type": "object" }),
        }];
        let raw = serde_json::to_string(&cached).unwrap();
        let back: Vec<CachedTool> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back[0].name, "notion-search");
        assert_eq!(tool_name(&back[0].name), "notion_search");
    }

    #[test]
    fn first_binding_is_deterministic() {
        let mut map = BindingMap::new();
        let a = ProfileId::from_slug("aaa");
        let b = ProfileId::from_slug("bbb");
        map.insert(
            a,
            IntegrationBinding::from_config(json!({ "account": "one" })),
        );
        map.insert(
            b,
            IntegrationBinding::from_config(json!({ "account": "two" })),
        );
        let (first, _) = first_binding(&map).unwrap();
        let expected = if a.to_string() < b.to_string() { a } else { b };
        assert_eq!(first, expected);
    }
}

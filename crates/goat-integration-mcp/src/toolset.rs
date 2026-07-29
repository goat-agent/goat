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
use tracing::warn;

use crate::McpService;

const TOOLS_STREAM: &str = "tools";
const MAX_TOOL_NAME_LEN: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDisposition {
    Enabled,
    Deferred,
    Skip,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl From<goat_mcp::McpTool> for CachedTool {
    fn from(tool: goat_mcp::McpTool) -> Self {
        Self {
            name: tool.name.to_string(),
            description: tool.description.as_deref().unwrap_or_default().to_string(),
            input_schema: Value::Object(tool.input_schema.as_ref().clone()),
        }
    }
}

pub fn prefixed(prefix: &str, raw: &str) -> String {
    if prefix.is_empty() || raw.starts_with(prefix) {
        raw.to_owned()
    } else {
        format!("{prefix}{raw}")
    }
}

pub fn first_binding(bindings: &BindingMap) -> Option<(ProfileId, &IntegrationBinding)> {
    bindings
        .iter()
        .min_by_key(|(persona, _)| persona.to_string())
        .map(|(persona, binding)| (*persona, binding))
}

pub async fn register(
    service: &Arc<McpService>,
    registry: &mut ToolRegistry,
    runtime: &IntegrationRuntime,
    bindings: Arc<BindingMap>,
) -> Vec<ToolName> {
    let Some((persona, binding)) = first_binding(&bindings) else {
        return Vec::new();
    };
    let discovered = discover(service, runtime, persona, binding).await;
    let mut names = Vec::new();
    let mut enabled = 0usize;
    let mut deferred = 0usize;
    let mut skipped = 0usize;

    for tool in discovered {
        let disposition = (service.tool_filter)(&tool);
        if disposition == ToolDisposition::Skip {
            skipped += 1;
            continue;
        }
        let Some(name) = usable_name(service, &tool) else {
            skipped += 1;
            continue;
        };
        let is_enabled = disposition == ToolDisposition::Enabled;
        if is_enabled {
            enabled += 1;
        } else {
            deferred += 1;
        }
        registry.insert_handler(
            ToolSpec::new(name.clone(), tool.description, tool.input_schema),
            Arc::new(McpPassthrough {
                service: service.clone(),
                credentials: runtime.credentials.clone(),
                bindings: bindings.clone(),
                tool: tool.name,
            }),
            is_enabled,
        );
        names.push(name);
    }

    tracing::info!(
        integration = service.id.as_str(),
        enabled,
        deferred,
        skipped,
        "registered mcp tools",
    );
    names
}

fn usable_name(service: &McpService, tool: &CachedTool) -> Option<ToolName> {
    let candidate = prefixed(service.tool_prefix, &tool.name);
    if candidate.len() > MAX_TOOL_NAME_LEN {
        warn!(
            integration = service.id.as_str(),
            tool = %tool.name,
            "skipping mcp tool with an over-long name",
        );
        return None;
    }
    let Ok(name) = ToolName::new(candidate) else {
        warn!(
            integration = service.id.as_str(),
            tool = %tool.name,
            "skipping mcp tool with an unusable name",
        );
        return None;
    };
    Some(name)
}

async fn discover(
    service: &Arc<McpService>,
    runtime: &IntegrationRuntime,
    persona: ProfileId,
    binding: &IntegrationBinding,
) -> Vec<CachedTool> {
    match fetch_live(service, &runtime.credentials, binding).await {
        Ok(tools) => {
            if let Ok(raw) = serde_json::to_string(&tools)
                && let Err(e) = runtime
                    .save_state(persona, &service.id, &binding.account, TOOLS_STREAM, &raw)
                    .await
            {
                warn!(integration = service.id.as_str(), error = %e, "failed to cache tool list");
            }
            tools
        }
        Err(e) => {
            warn!(
                integration = service.id.as_str(),
                error = %e,
                "tool discovery failed; falling back to the cached list",
            );
            match runtime
                .load_state(persona, &service.id, &binding.account, TOOLS_STREAM)
                .await
            {
                Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
                _ => Vec::new(),
            }
        }
    }
}

async fn fetch_live(
    service: &Arc<McpService>,
    credentials: &CredentialStore,
    binding: &IntegrationBinding,
) -> goat_integration::IntegrationResult<Vec<CachedTool>> {
    let session = service.connect(credentials, binding).await?;
    let tools = session.list_tools().await;
    session.close().await;
    Ok(tools
        .map_err(|e| service.wire_error(&e))?
        .into_iter()
        .map(CachedTool::from)
        .collect())
}

struct McpPassthrough {
    service: Arc<McpService>,
    credentials: CredentialStore,
    bindings: Arc<BindingMap>,
    tool: String,
}

#[async_trait]
impl ToolHandler for McpPassthrough {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let name = self.service.id.as_str();
        let Some(binding) = self.bindings.get(&ctx.persona) else {
            return ToolOutput::error(format!(
                "{name} is not configured for this agent; run `goat agent integration add {name}`"
            ));
        };
        let session = match self.service.connect(&self.credentials, binding).await {
            Ok(session) => session,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        let result = self
            .service
            .call(&session, &self.tool, drop_placeholder_args(call.arguments))
            .await;
        session.close().await;
        match result {
            Ok(value) => ToolOutput::structured(value),
            Err(e) => ToolOutput::error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn service() -> McpService {
        McpService::new("acme", "Acme", "https://mcp.acme.test/mcp", "setup")
            .with_tool_prefix("acme_")
    }

    fn tool(name: &str) -> CachedTool {
        CachedTool {
            name: name.to_owned(),
            description: String::new(),
            input_schema: json!({ "type": "object" }),
        }
    }

    #[test]
    fn the_prefix_is_added_once_and_never_doubled() {
        assert_eq!(prefixed("acme_", "search"), "acme_search");
        assert_eq!(prefixed("acme_", "acme_search"), "acme_search");
        assert_eq!(prefixed("", "search"), "search");
    }

    #[test]
    fn an_over_long_name_is_skipped_rather_than_registered_broken() {
        let service = service();
        assert!(usable_name(&service, &tool("x")).is_some());
        assert!(usable_name(&service, &tool(&"x".repeat(MAX_TOOL_NAME_LEN))).is_none());
    }

    #[test]
    fn a_name_the_registry_would_reject_is_skipped() {
        let service = service();
        assert!(usable_name(&service, &tool("bad name!")).is_none());
    }

    #[test]
    fn first_binding_is_deterministic() {
        let mut bindings = BindingMap::new();
        let a = ProfileId::from_slug("aaa");
        let b = ProfileId::from_slug("bbb");
        bindings.insert(a, IntegrationBinding::from_config(json!({})));
        bindings.insert(b, IntegrationBinding::from_config(json!({})));
        let first = first_binding(&bindings).unwrap().0;
        assert_eq!(first, first_binding(&bindings).unwrap().0);
        assert!(first == a || first == b);
    }

    #[test]
    fn a_cached_tool_survives_a_json_round_trip() {
        let original = tool("search");
        let raw = serde_json::to_string(std::slice::from_ref(&original)).unwrap();
        let back: Vec<CachedTool> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back[0], original);
    }
}

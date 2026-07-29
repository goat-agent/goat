use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolContext, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_auth::CredentialStore;
use goat_integration::{
    BindingMap, IntegrationBinding, IntegrationError, IntegrationRuntime, drop_placeholder_args,
};
use goat_types::ProfileId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::{ID, mcp};

const TOOLS_STREAM: &str = "tools";
const PREFIX: &str = "tiro_";

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

pub fn tool_name(raw: &str) -> String {
    if raw.starts_with(PREFIX) {
        raw.to_string()
    } else {
        format!("{PREFIX}{raw}")
    }
}

pub fn client_id_of(binding: &IntegrationBinding) -> Option<String> {
    binding
        .config
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::to_string)
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
            warn!(tool = %tool.name, "skipping tiro mcp tool with unusable name");
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
                warn!(error = %e, "failed to cache tiro tool list");
            }
            tools
        }
        Err(e @ (IntegrationError::Auth(_) | IntegrationError::Config(_))) => {
            warn!(error = %e, "tiro tool discovery was rejected; registering nothing");
            Vec::new()
        }
        Err(e) => {
            warn!(error = %e, "tiro tool discovery failed; using cached list");
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
    let client_id = client_id_of(binding);
    let auth = mcp::resolve_auth(credentials, &binding.account, client_id.as_deref())?;
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
                "tiro is not configured for this agent; run `goat agent integration add tiro`",
            );
        };
        let client_id = client_id_of(binding);
        let auth =
            match mcp::resolve_auth(&self.credentials, &binding.account, client_id.as_deref()) {
                Ok(auth) => auth,
                Err(e) => return ToolOutput::error(e.to_string()),
            };
        let session = match mcp::connect(&auth).await {
            Ok(session) => session,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        let result = session
            .call(&self.tool, drop_placeholder_args(call.arguments))
            .await;
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
    fn vendor_prefix_is_added_once_and_never_doubled() {
        assert_eq!(tool_name("list_notes"), "tiro_list_notes");
        assert_eq!(tool_name("tiro_list_notes"), "tiro_list_notes");
        assert!(ToolName::new(tool_name("get_note_transcript")).is_ok());
        assert!(ToolName::new(tool_name("bad.name")).is_err());
    }

    #[test]
    fn cached_tools_round_trip() {
        let cached = vec![CachedTool {
            name: "list_notes".into(),
            description: "List notes by filter and optional keyword".into(),
            input_schema: json!({ "type": "object" }),
        }];
        let raw = serde_json::to_string(&cached).unwrap();
        let back: Vec<CachedTool> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back[0].name, "list_notes");
        assert_eq!(tool_name(&back[0].name), "tiro_list_notes");
    }

    #[test]
    fn client_id_comes_from_the_merged_binding() {
        let bound = IntegrationBinding::from_config(json!({ "client_id": "abc" }));
        assert_eq!(client_id_of(&bound), Some("abc".to_string()));
        let bare = IntegrationBinding::from_config(json!({}));
        assert_eq!(client_id_of(&bare), None);
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

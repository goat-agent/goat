use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolContext, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_auth::CredentialStore;
use goat_integration::{BindingMap, IntegrationBinding, IntegrationError, IntegrationRuntime};
use goat_types::ProfileId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::{ID, mcp};

const TOOLS_STREAM: &str = "tools";
const PREFIX: &str = "posthog_";

const MAX_TOOL_NAME_LEN: usize = 64;

pub const DENY_SUFFIXES: &[&str] = &["-delete", "-destroy"];

pub const ENABLED_TOOLS: &[&str] = &[
    "execute-sql",
    "insight-query",
    "docs-search",
    "project-get",
    "organization-get",
    "query-error-tracking-issues-list",
    "query-error-tracking-issue",
    "query-error-tracking-issue-events",
    "feature-flag-get-all",
    "feature-flag-get-definition",
    "feature-flag-get-definition-by-key",
    "feature-flags-status-retrieve",
    "create-feature-flag",
    "update-feature-flag",
    "query-logs",
    "logs-count",
    "logs-patterns",
    "dashboards-get-all",
    "dashboard-get",
    "dashboard-insights-run",
    "annotations-list",
    "annotation-create",
    "get-llm-total-costs-for-project",
    "llma-personal-spend",
    "experiment-get-all",
    "experiment-get",
    "experiment-results-get",
];

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

pub fn is_denied(raw: &str, deny_suffixes: &[String]) -> bool {
    deny_suffixes.iter().any(|suffix| raw.ends_with(suffix))
}

pub fn schema_is_usable(schema: &Value) -> bool {
    schema.is_object()
        && serde_json::to_string(schema)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .is_some_and(|back| &back == schema)
}

pub fn deny_suffixes(config: &Value) -> Vec<String> {
    config
        .get("deny_suffixes")
        .and_then(Value::as_array)
        .map_or_else(
            || DENY_SUFFIXES.iter().map(|s| (*s).to_string()).collect(),
            |values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            },
        )
}

pub struct Registration {
    pub enabled: Vec<ToolName>,
    pub deferred: Vec<ToolName>,
    pub skipped: usize,
}

pub async fn register(
    registry: &mut ToolRegistry,
    runtime: &IntegrationRuntime,
    bindings: Arc<BindingMap>,
) -> Vec<ToolName> {
    let Some((persona, binding)) = first_binding(&bindings) else {
        return Vec::new();
    };
    let catalogue = discover(runtime, persona, binding).await;
    let deny = deny_suffixes(&binding.config);
    let outcome = insert_all(registry, &catalogue, &deny, &bindings, runtime);
    info!(
        catalogue = catalogue.len(),
        enabled = outcome.enabled.len(),
        deferred = outcome.deferred.len(),
        skipped = outcome.skipped,
        "registered posthog tools"
    );
    for wanted in ENABLED_TOOLS {
        if !catalogue.iter().any(|tool| tool.name == *wanted) {
            warn!(
                tool = wanted,
                "posthog catalogue is missing an enabled tool"
            );
        }
    }
    outcome.enabled
}

fn insert_all(
    registry: &mut ToolRegistry,
    catalogue: &[CachedTool],
    deny: &[String],
    bindings: &Arc<BindingMap>,
    runtime: &IntegrationRuntime,
) -> Registration {
    let mut outcome = Registration {
        enabled: Vec::new(),
        deferred: Vec::new(),
        skipped: 0,
    };
    for tool in catalogue {
        let Some(name) = usable_name(tool, deny) else {
            outcome.skipped += 1;
            continue;
        };
        let enabled = ENABLED_TOOLS.contains(&tool.name.as_str());
        registry.insert_handler(
            ToolSpec::new(
                name.clone(),
                tool.description.clone(),
                tool.input_schema.clone(),
            ),
            Arc::new(McpPassthrough {
                credentials: runtime.credentials.clone(),
                bindings: bindings.clone(),
                tool: tool.name.clone(),
            }),
            enabled,
        );
        if enabled {
            outcome.enabled.push(name);
        } else {
            outcome.deferred.push(name);
        }
    }
    outcome
}

fn usable_name(tool: &CachedTool, deny: &[String]) -> Option<ToolName> {
    if is_denied(&tool.name, deny) {
        warn!(tool = %tool.name, "skipping destructive posthog tool");
        return None;
    }
    if !schema_is_usable(&tool.input_schema) {
        warn!(tool = %tool.name, "skipping posthog tool with an unusable input schema");
        return None;
    }
    let prefixed = tool_name(&tool.name);
    if prefixed.len() > MAX_TOOL_NAME_LEN {
        warn!(
            tool = %tool.name,
            len = prefixed.len(),
            "skipping posthog tool whose prefixed name exceeds the provider name limit"
        );
        return None;
    }
    let Ok(name) = ToolName::new(prefixed) else {
        warn!(tool = %tool.name, "skipping posthog tool with unusable name");
        return None;
    };
    Some(name)
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
                warn!(error = %e, "failed to cache posthog tool list");
            }
            tools
        }
        Err(e @ IntegrationError::Auth(_)) => {
            warn!(error = %e, "posthog tool discovery failed on auth; registering nothing");
            Vec::new()
        }
        Err(e) => {
            warn!(error = %e, "posthog tool discovery failed; using cached list");
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
    let auth = mcp::resolve_auth(credentials, &binding.account, client_id(&binding.config))?;
    let scope = mcp::ProjectScope::from_config(&binding.config);
    let session = mcp::connect(&auth, &scope).await?;
    let tools = session.list_tools().await;
    mcp::persist_tokens(credentials, &binding.account, &session).await;
    session.close().await;
    Ok(tools?.into_iter().map(CachedTool::from).collect())
}

fn client_id(config: &Value) -> Option<&str> {
    config.get("client_id").and_then(Value::as_str)
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
                "posthog is not configured for this agent; run `goat agent integration add posthog`",
            );
        };
        let auth = match mcp::resolve_auth(
            &self.credentials,
            &binding.account,
            client_id(&binding.config),
        ) {
            Ok(auth) => auth,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        let scope = mcp::ProjectScope::from_config(&binding.config);
        let session = match mcp::connect(&auth, &scope).await {
            Ok(session) => session,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        let result = session.call(&self.tool, call.arguments).await;
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

    fn tool(name: &str) -> CachedTool {
        CachedTool {
            name: name.to_string(),
            description: "d".into(),
            input_schema: json!({ "type": "object" }),
        }
    }

    #[test]
    fn vendor_prefix_is_added_once_and_never_doubled() {
        assert_eq!(tool_name("execute-sql"), "posthog_execute-sql");
        assert_eq!(tool_name("posthog_execute-sql"), "posthog_execute-sql");
        assert!(ToolName::new(tool_name("feature-flag-get-all")).is_ok());
        assert!(ToolName::new(tool_name("bad.name")).is_err());
    }

    #[test]
    fn destructive_tools_are_denied() {
        let deny: Vec<String> = DENY_SUFFIXES.iter().map(|s| (*s).to_string()).collect();
        assert!(is_denied("dashboard-delete", &deny));
        assert!(is_denied("notebooks-destroy", &deny));
        assert!(!is_denied("dashboard-get", &deny));
        assert!(usable_name(&tool("dashboard-delete"), &deny).is_none());
        assert!(usable_name(&tool("dashboard-get"), &deny).is_some());
    }

    #[test]
    fn deny_suffixes_fall_back_to_the_default_list() {
        assert_eq!(deny_suffixes(&json!({})), vec!["-delete", "-destroy"]);
        assert_eq!(
            deny_suffixes(&json!({ "deny_suffixes": ["-nuke"] })),
            vec!["-nuke"]
        );
    }

    #[test]
    fn overlong_and_malformed_tools_are_skipped() {
        let deny: Vec<String> = DENY_SUFFIXES.iter().map(|s| (*s).to_string()).collect();

        let long = tool(&"a".repeat(MAX_TOOL_NAME_LEN));
        assert!(tool_name(&long.name).len() > MAX_TOOL_NAME_LEN);
        assert!(usable_name(&long, &deny).is_none());

        let mut bad_schema = tool("execute-sql");
        bad_schema.input_schema = json!("not an object");
        assert!(usable_name(&bad_schema, &deny).is_none());

        assert!(usable_name(&tool("execute-sql"), &deny).is_some());
    }

    #[test]
    fn schema_round_trip_check_accepts_real_schemas() {
        assert!(schema_is_usable(&json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        })));
        assert!(!schema_is_usable(&json!([1, 2, 3])));
        assert!(!schema_is_usable(&json!(null)));
    }

    #[test]
    fn enabled_list_is_disjoint_from_the_deny_list() {
        let deny: Vec<String> = DENY_SUFFIXES.iter().map(|s| (*s).to_string()).collect();
        for name in ENABLED_TOOLS {
            assert!(!is_denied(name, &deny), "{name} is both enabled and denied");
            assert!(
                tool_name(name).len() <= MAX_TOOL_NAME_LEN,
                "{name} exceeds the provider name limit once prefixed"
            );
        }
    }

    #[test]
    fn cached_tools_round_trip() {
        let cached = vec![tool("execute-sql")];
        let raw = serde_json::to_string(&cached).unwrap();
        let back: Vec<CachedTool> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back[0].name, "execute-sql");
        assert_eq!(tool_name(&back[0].name), "posthog_execute-sql");
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

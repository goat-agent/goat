use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_tool::{
    ToolCall, ToolCaller, ToolHandler, ToolName, ToolOutput, ToolRegistry, ToolSpec,
};
use goat_auth::CredentialStore;
use goat_integration::{BindingMap, IntegrationBinding, IntegrationRuntime, drop_placeholder_args};
use goat_types::AgentId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::{Enable, McpService, NameRule, ToolPolicy};

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

pub fn normalized(prefix: &str, name: &str) -> String {
    let flat = name.replace('-', "_");
    let flat_prefix = prefix.replace('-', "_");
    flat.strip_prefix(&flat_prefix).unwrap_or(&flat).to_owned()
}

pub fn pick_tool<'a, I>(available: I, candidates: &[&str], prefix: &str) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let available: Vec<&str> = available.into_iter().collect();
    candidates.iter().find_map(|candidate| {
        let want = normalized(prefix, candidate);
        available
            .iter()
            .find(|name| normalized(prefix, name) == want)
            .map(|name| (*name).to_owned())
    })
}

pub fn first_binding(bindings: &BindingMap) -> Option<(AgentId, &IntegrationBinding)> {
    bindings
        .iter()
        .min_by_key(|(agent, _)| agent.to_string())
        .map(|(agent, binding)| (*agent, binding))
}

pub async fn register(
    service: &Arc<McpService>,
    registry: &mut ToolRegistry,
    runtime: &IntegrationRuntime,
    bindings: Arc<BindingMap>,
) -> Vec<ToolName> {
    let Some((agent, binding)) = first_binding(&bindings) else {
        return Vec::new();
    };
    let discovered = discover(service, runtime, agent, binding).await;
    let deny = DenyRules::effective(&service.tools, &binding.config);
    let mut names = Vec::new();
    let mut enabled = 0usize;
    let mut deferred = 0usize;
    let mut skipped = 0usize;

    for tool in &discovered {
        let disposition = disposition(service, &service.tools, &deny, tool);
        if disposition == ToolDisposition::Skip {
            skipped += 1;
            continue;
        }
        let Some(name) = usable_name(service, tool) else {
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
            ToolSpec::new(
                name.clone(),
                tool.description.clone(),
                tool.input_schema.clone(),
            ),
            Arc::new(McpPassthrough {
                service: service.clone(),
                credentials: runtime.credentials.clone(),
                bindings: bindings.clone(),
                tool: tool.name.clone(),
            }),
            is_enabled,
        );
        names.push(name);
    }

    warn_about_missing_tools(service, &discovered);
    tracing::info!(
        integration = service.id.as_str(),
        enabled,
        deferred,
        skipped,
        "registered mcp tools",
    );
    names
}

fn disposition(
    service: &McpService,
    policy: &ToolPolicy,
    deny: &DenyRules,
    tool: &CachedTool,
) -> ToolDisposition {
    if deny.hits(&tool.name) {
        warn!(
            integration = service.id.as_str(),
            tool = %tool.name,
            "skipping a denied mcp tool",
        );
        return ToolDisposition::Skip;
    }
    if !schema_is_usable(&tool.input_schema) {
        warn!(
            integration = service.id.as_str(),
            tool = %tool.name,
            "skipping an mcp tool with an unusable input schema",
        );
        return ToolDisposition::Skip;
    }
    match policy.enable {
        Enable::All => ToolDisposition::Enabled,
        Enable::Only(wanted) => {
            if wanted.iter().any(|want| {
                normalized(policy.prefix, want) == normalized(policy.prefix, &tool.name)
            }) {
                ToolDisposition::Enabled
            } else {
                ToolDisposition::Deferred
            }
        }
    }
}

fn warn_about_missing_tools(service: &McpService, discovered: &[CachedTool]) {
    let Enable::Only(wanted) = service.tools.enable else {
        return;
    };
    if discovered.is_empty() {
        return;
    }
    let prefix = service.tools.prefix;
    for want in wanted {
        if !discovered
            .iter()
            .any(|tool| normalized(prefix, &tool.name) == normalized(prefix, want))
        {
            warn!(
                integration = service.id.as_str(),
                tool = want,
                "the catalogue is missing an enabled tool; the server may be outdated",
            );
        }
    }
}

struct DenyRules {
    prefixes: Vec<String>,
    suffixes: Vec<String>,
}

impl DenyRules {
    fn effective(policy: &ToolPolicy, config: &Value) -> Self {
        let mut prefixes: Vec<String> = policy
            .deny
            .iter()
            .filter_map(|rule| match rule {
                NameRule::Prefix(prefix) => Some((*prefix).to_owned()),
                NameRule::Suffix(_) => None,
            })
            .collect();
        let mut suffixes: Vec<String> = policy
            .deny
            .iter()
            .filter_map(|rule| match rule {
                NameRule::Suffix(suffix) => Some((*suffix).to_owned()),
                NameRule::Prefix(_) => None,
            })
            .collect();
        if let Some(overridden) = string_list(config, "deny_prefixes") {
            prefixes = overridden;
        }
        if let Some(overridden) = string_list(config, "deny_suffixes") {
            suffixes = overridden;
        }
        Self { prefixes, suffixes }
    }

    fn hits(&self, name: &str) -> bool {
        self.prefixes.iter().any(|prefix| name.starts_with(prefix))
            || self.suffixes.iter().any(|suffix| name.ends_with(suffix))
    }
}

fn string_list(config: &Value, key: &str) -> Option<Vec<String>> {
    config.get(key)?.as_array().map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

fn schema_is_usable(schema: &Value) -> bool {
    schema.is_object()
        && serde_json::to_string(schema)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .is_some_and(|back| &back == schema)
}

fn usable_name(service: &McpService, tool: &CachedTool) -> Option<ToolName> {
    let candidate = prefixed(service.tools.prefix, &tool.name);
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
    agent: AgentId,
    binding: &IntegrationBinding,
) -> Vec<CachedTool> {
    match fetch_live(service, &runtime.credentials, binding).await {
        Ok(tools) => {
            if let Ok(raw) = serde_json::to_string(&tools)
                && let Err(e) = runtime
                    .save_state(agent, &service.id, &binding.account, TOOLS_STREAM, &raw)
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
                .load_state(agent, &service.id, &binding.account, TOOLS_STREAM)
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
    async fn call(&self, ctx: ToolCaller, call: ToolCall) -> ToolOutput {
        let name = self.service.id.as_str();
        let Some(binding) = self.bindings.get(&ctx.agent) else {
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
    use crate::ServiceUrl;
    use serde_json::json;

    fn service() -> McpService {
        McpService::new(
            "acme",
            "Acme",
            ServiceUrl::Fixed("https://mcp.acme.test/mcp"),
            "setup",
        )
        .tools(ToolPolicy::all("acme_"))
    }

    fn tool(name: &str) -> CachedTool {
        CachedTool {
            name: name.to_owned(),
            description: String::new(),
            input_schema: json!({ "type": "object" }),
        }
    }

    fn classify(service: &McpService, config: &Value, name: &str) -> ToolDisposition {
        let deny = DenyRules::effective(&service.tools, config);
        disposition(service, &service.tools, &deny, &tool(name))
    }

    #[test]
    fn the_prefix_is_added_once_and_never_doubled() {
        assert_eq!(prefixed("acme_", "search"), "acme_search");
        assert_eq!(prefixed("acme_", "acme_search"), "acme_search");
        assert_eq!(prefixed("", "search"), "search");
    }

    #[test]
    fn a_prefix_is_stripped_exactly_once() {
        assert_eq!(normalized("slack_", "slack_search_public"), "search_public");
        assert_eq!(
            normalized("slack_", "slack_slack_search_public"),
            "slack_search_public"
        );
    }

    #[test]
    fn hyphen_and_underscore_spellings_are_the_same_tool() {
        assert_eq!(
            normalized("notion_", "notion-query-data-sources"),
            "query_data_sources"
        );
        assert_eq!(
            normalized("notion_", "notion_query_data_sources"),
            "query_data_sources"
        );
    }

    #[test]
    fn the_first_matching_candidate_wins() {
        let available = ["slack_search_messages", "slack_search_public"];
        let picked = pick_tool(
            available.iter().copied(),
            &[
                "search_public_and_private",
                "search_messages",
                "search_public",
            ],
            "slack_",
        );
        assert_eq!(picked.as_deref(), Some("slack_search_messages"));
    }

    #[test]
    fn a_hyphenated_remote_name_is_still_found() {
        let available = ["notion-query-data-sources"];
        let picked = pick_tool(
            available.iter().copied(),
            &["query_data_sources"],
            "notion_",
        );
        assert_eq!(picked.as_deref(), Some("notion-query-data-sources"));
    }

    #[test]
    fn nothing_matching_resolves_to_none() {
        assert!(pick_tool(["a", "b"].iter().copied(), &["c"], "x_").is_none());
    }

    #[test]
    fn an_all_policy_enables_everything_not_denied() {
        let service = service();
        assert_eq!(
            classify(&service, &json!({}), "search"),
            ToolDisposition::Enabled
        );
    }

    #[test]
    fn an_only_policy_defers_what_it_does_not_name() {
        let service = McpService::new("acme", "Acme", ServiceUrl::Fixed("u"), "s")
            .tools(ToolPolicy::only("acme_", &["execute-sql", "listThings"]));
        assert_eq!(
            classify(&service, &json!({}), "execute_sql"),
            ToolDisposition::Enabled
        );
        assert_eq!(
            classify(&service, &json!({}), "listThings"),
            ToolDisposition::Enabled
        );
        assert_eq!(
            classify(&service, &json!({}), "dropThings"),
            ToolDisposition::Deferred
        );
    }

    #[test]
    fn deny_rules_skip_by_prefix_and_suffix() {
        let service = McpService::new("acme", "Acme", ServiceUrl::Fixed("u"), "s").tools(
            ToolPolicy::all("acme_")
                .deny(&[NameRule::Prefix("delete"), NameRule::Suffix("-destroy")]),
        );
        assert_eq!(
            classify(&service, &json!({}), "deleteThing"),
            ToolDisposition::Skip
        );
        assert_eq!(
            classify(&service, &json!({}), "thing-destroy"),
            ToolDisposition::Skip
        );
        assert_eq!(
            classify(&service, &json!({}), "getThing"),
            ToolDisposition::Enabled
        );
    }

    #[test]
    fn the_binding_replaces_deny_rules_of_the_same_kind() {
        let service = McpService::new("acme", "Acme", ServiceUrl::Fixed("u"), "s")
            .tools(ToolPolicy::all("acme_").deny(&[NameRule::Suffix("-delete")]));
        let config = json!({ "deny_suffixes": ["-nuke"] });
        assert_eq!(
            classify(&service, &config, "thing-delete"),
            ToolDisposition::Enabled
        );
        assert_eq!(
            classify(&service, &config, "thing-nuke"),
            ToolDisposition::Skip
        );
    }

    #[test]
    fn a_broken_schema_is_skipped() {
        let service = service();
        let deny = DenyRules::effective(&service.tools, &json!({}));
        let broken = CachedTool {
            name: "x".to_owned(),
            description: String::new(),
            input_schema: json!("not an object"),
        };
        assert_eq!(
            disposition(&service, &service.tools, &deny, &broken),
            ToolDisposition::Skip
        );
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
        let a = AgentId::from_slug("aaa");
        let b = AgentId::from_slug("bbb");
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

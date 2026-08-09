use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::SETTLE;
use goat_integration::query::{self, TokenValue};
use goat_integration::shape;
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_integration_mcp::{McpService, pick_tool};
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::{PREFIX, VOCABULARY, service};

pub const STREAM: &str = "monitors";
pub const DEFAULT_QUERY: &str = "state:alert";

pub const LIST_TOOL_CANDIDATES: &[&str] = &[
    "get-monitors",
    "get_monitors",
    "list_monitors",
    "list-monitors",
    "search_monitors",
    "search-monitors",
];

const SUMMARY_LIMIT: usize = 160;
const DEFAULT_LIMIT: usize = 50;

pub fn defaults(_: &IntegrationBinding) -> Vec<WatchSpec> {
    vec![WatchSpec {
        state_key: STREAM.to_owned(),
        stream: STREAM.to_owned(),
        query: DEFAULT_QUERY.to_owned(),
    }]
}

pub fn compile(
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    spec: &WatchSpec,
) -> IntegrationResult<CompiledWatch> {
    let plan = plan(&spec.query)?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Updated,
        entity: "monitor",
        diff: SETTLE,
        source: Box::new(MonitorSearch {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            resolved_tool: OnceCell::new(),
            arguments: plan.arguments,
            states: plan.states,
        }),
    })
}

struct Plan {
    arguments: Value,
    states: Vec<String>,
}

fn plan(raw: &str) -> IntegrationResult<Plan> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let states: Vec<String> = resolved
        .many("state")
        .filter(|m| !m.negated)
        .filter_map(|m| match &m.value {
            TokenValue::Text(text) => Some(text.clone()),
            TokenValue::SelfRef => None,
        })
        .collect();
    let tags: Vec<String> = resolved
        .many("tag")
        .filter_map(|m| match &m.value {
            TokenValue::Text(text) => Some(text.clone()),
            TokenValue::SelfRef => None,
        })
        .collect();
    let mut arguments = json!({ "page_size": resolved.limit.unwrap_or(DEFAULT_LIMIT) });
    if !states.is_empty() {
        arguments["group_states"] = json!(states.join(","));
    }
    if !tags.is_empty() {
        arguments["monitor_tags"] = json!(tags.join(","));
    }
    if let Some(matched) = resolved.single("name")
        && let TokenValue::Text(name) = &matched.value
    {
        arguments["name"] = json!(name);
    }
    Ok(Plan { arguments, states })
}

struct MonitorSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    resolved_tool: OnceCell<String>,
    arguments: Value,
    states: Vec<String>,
}

impl MonitorSearch {
    async fn list_tool(&self, session: &goat_mcp::McpSession) -> IntegrationResult<String> {
        if let Some(tool) = self.resolved_tool.get() {
            return Ok(tool.clone());
        }
        let available = session
            .list_tools()
            .await
            .map_err(|e| self.service.wire_error(&e))?;
        let names: Vec<String> = available.iter().map(|t| t.name.to_string()).collect();
        let picked = pick_tool(
            names.iter().map(String::as_str),
            LIST_TOOL_CANDIDATES,
            PREFIX,
        )
        .ok_or_else(|| {
            IntegrationError::Config(format!(
                "the datadog mcp exposes no recognized monitor list tool (available: {})",
                names.join(", ")
            ))
        })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }
}

impl WatchSource for MonitorSearch {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let tool = self.list_tool(&session).await;
        let result = match tool {
            Ok(tool) => {
                self.service
                    .call(&session, &tool, self.arguments.clone())
                    .await
            }
            Err(e) => Err(e),
        };
        session.close().await;
        let value = result?;
        let items = shape::items("datadog", &value, &["monitors"])?
            .iter()
            .filter(|node| self.keeps(node))
            .map(observed)
            .collect::<IntegrationResult<Vec<_>>>()?;
        Ok(WatchPage {
            items,
            truncated: Some(shape::more(&value)),
        })
    }
}

impl MonitorSearch {
    fn keeps(&self, node: &Value) -> bool {
        if self.states.is_empty() {
            return true;
        }
        let state = state_of(node);
        self.states.iter().any(|wanted| wanted == &state)
    }
}

fn state_of(node: &Value) -> String {
    shape::text(node, &["overall_state", "overall_status", "state"]).to_lowercase()
}

fn observed(node: &Value) -> IntegrationResult<Observed> {
    let key = shape::required("datadog", node, &["id", "public_id"])?;
    let state = state_of(node);
    let modified = shape::text(
        node,
        &[
            "overall_state_modified",
            "modified",
            "modified_at",
            "created",
        ],
    );
    let name = shape::squeeze(&shape::text(node, &["name", "title"]), SUMMARY_LIMIT);
    Ok(Observed::new(
        key.clone(),
        format!("{state}:{modified}"),
        format!("[{state}] {name}"),
        node.clone(),
    )
    .with_reference(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_default_query_asks_for_alerting_groups() {
        let plan = plan(DEFAULT_QUERY).unwrap();
        assert_eq!(plan.arguments["group_states"], json!("alert"));
        assert_eq!(plan.states, vec!["alert".to_owned()]);
    }

    #[test]
    fn tags_and_name_reach_the_request() {
        let plan = plan("tag:env:prod tag:team:sre name:latency").unwrap();
        assert_eq!(plan.arguments["monitor_tags"], json!("env:prod,team:sre"));
        assert_eq!(plan.arguments["name"], json!("latency"));
    }

    #[test]
    fn unknown_keys_and_free_text_are_refused() {
        assert!(plan("service:api").is_err());
        assert!(plan("just words").is_err());
        assert!(plan("state:nonsense").is_err());
    }

    #[test]
    fn the_state_is_re_checked_client_side_because_the_api_filters_by_group() {
        let search = |states: Vec<String>| MonitorSearch {
            service: Arc::new(service()),
            credentials: CredentialStore::new(std::path::PathBuf::from("/dev/null")),
            binding: IntegrationBinding::from_config(json!({})),
            resolved_tool: OnceCell::new(),
            arguments: json!({}),
            states,
        };
        let alerting = json!({ "id": 1, "overall_state": "Alert" });
        let ok = json!({ "id": 2, "overall_state": "OK" });
        let only_alert = search(vec!["alert".to_owned()]);
        assert!(only_alert.keeps(&alerting));
        assert!(!only_alert.keeps(&ok));
        assert!(search(Vec::new()).keeps(&ok));
    }

    #[test]
    fn the_stamp_carries_the_state_so_a_transition_re_fires() {
        let alerting = observed(&json!({
            "id": 7,
            "name": "checkout   p99",
            "overall_state": "Alert",
            "overall_state_modified": "2026-08-08T01:00:00Z"
        }))
        .unwrap();
        let recovered = observed(&json!({
            "id": 7,
            "name": "checkout p99",
            "overall_state": "OK",
            "overall_state_modified": "2026-08-08T01:00:00Z"
        }))
        .unwrap();
        assert_eq!(alerting.key, recovered.key);
        assert_ne!(alerting.stamp, recovered.stamp);
        assert_eq!(alerting.summary, "[alert] checkout p99");
    }

    #[test]
    fn a_monitor_without_an_id_is_an_error() {
        assert!(observed(&json!({ "name": "x" })).is_err());
    }
}

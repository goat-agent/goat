use std::fmt::Write as _;
use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::RETAIN;
use goat_integration::query::{self, ResolvedQuery, TokenValue};
use goat_integration::shape;
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_integration_mcp::{McpService, pick_tool};
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::{PREFIX, PagerdutyBinding, VOCABULARY, service};

pub const STREAM: &str = "oncall";
pub const DEFAULT_QUERY: &str = "is:triggered";

pub const LIST_TOOL_CANDIDATES: &[&str] = &[
    "list_incidents",
    "get_incidents",
    "incidents_list",
    "list-incidents",
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
    let arguments = plan(
        &spec.query,
        PagerdutyBinding::read(&binding.config).user_id.as_deref(),
    )?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Assigned,
        entity: "incident",
        diff: RETAIN,
        source: Box::new(IncidentSearch {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            resolved_tool: OnceCell::new(),
            arguments,
        }),
    })
}

fn plan(raw: &str, user_id: Option<&str>) -> IntegrationResult<Value> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let mut arguments = json!({ "limit": resolved.limit.unwrap_or(DEFAULT_LIMIT) });
    let statuses = statuses_of(&resolved);
    if !statuses.is_empty() {
        arguments["statuses"] = json!(statuses);
    }
    for (key, field) in [
        ("service", "service_ids"),
        ("urgency", "urgencies"),
        ("assignee", "user_ids"),
    ] {
        let mut values = Vec::new();
        for matched in resolved.many(key) {
            match &matched.value {
                TokenValue::SelfRef => {
                    let me = user_id.ok_or_else(|| {
                        IntegrationError::Config(
                            "pagerduty needs `user_id` in the agent's pagerduty binding \
                             before `assignee:@me` can resolve"
                                .to_owned(),
                        )
                    })?;
                    values.push(me.to_owned());
                }
                TokenValue::Text(text) => values.push(text.clone()),
            }
        }
        if !values.is_empty() {
            arguments[field] = json!(values);
        }
    }
    Ok(arguments)
}

fn statuses_of(resolved: &ResolvedQuery) -> Vec<String> {
    let mut statuses: Vec<String> = resolved
        .many("status")
        .filter(|m| !m.negated)
        .filter_map(|m| match &m.value {
            TokenValue::Text(text) => Some(text.clone()),
            TokenValue::SelfRef => None,
        })
        .collect();
    if resolved.state("triggered") == Some(true) {
        statuses.push("triggered".to_owned());
    }
    if resolved.state("open") == Some(true) {
        statuses.push("triggered".to_owned());
        statuses.push("acknowledged".to_owned());
    }
    if resolved.state("resolved") == Some(true) {
        statuses.push("resolved".to_owned());
    }
    statuses.sort();
    statuses.dedup();
    statuses
}

struct IncidentSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    resolved_tool: OnceCell<String>,
    arguments: Value,
}

impl IncidentSearch {
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
                "the pagerduty mcp exposes no recognized incident list tool (available: {})",
                names.join(", ")
            ))
        })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }
}

impl WatchSource for IncidentSearch {
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
        let items = shape::items("pagerduty", &value, &["incidents"])?
            .iter()
            .map(observed)
            .collect::<IntegrationResult<Vec<_>>>()?;
        Ok(WatchPage {
            items,
            truncated: Some(shape::more(&value)),
        })
    }
}

fn observed(node: &Value) -> IntegrationResult<Observed> {
    let key = shape::required("pagerduty", node, &["id", "incident_key"])?;
    let stamp = shape::required(
        "pagerduty",
        node,
        &["last_status_change_at", "updated_at", "created_at"],
    )?;
    let number = shape::text(node, &["incident_number"]);
    let title = shape::squeeze(
        &shape::text(node, &["title", "summary", "description"]),
        SUMMARY_LIMIT,
    );
    let status = shape::text(node, &["status"]);
    let service = shape::text(node, &["service.summary", "service.name"]);
    let reference = if number.is_empty() {
        key.clone()
    } else {
        format!("#{number}")
    };
    let mut summary = format!("{reference} [{status}] — {title}");
    if !service.is_empty() {
        let _ = write!(summary, " · {service}");
    }
    Ok(Observed::new(key, stamp, summary, node.clone()).with_reference(reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_default_query_asks_only_for_triggered_incidents() {
        let args = plan(DEFAULT_QUERY, None).unwrap();
        assert_eq!(args["statuses"], json!(["triggered"]));
        assert_eq!(args["limit"], json!(DEFAULT_LIMIT));
    }

    #[test]
    fn is_open_covers_triggered_and_acknowledged() {
        let args = plan("is:open", None).unwrap();
        assert_eq!(args["statuses"], json!(["acknowledged", "triggered"]));
    }

    #[test]
    fn self_reference_needs_a_user_id() {
        assert!(plan("assignee:@me", None).is_err());
        let args = plan("assignee:@me", Some("PABC123")).unwrap();
        assert_eq!(args["user_ids"], json!(["PABC123"]));
    }

    #[test]
    fn unknown_keys_are_refused_rather_than_forwarded() {
        assert!(plan("sprint:current", None).is_err());
        assert!(plan("free text", None).is_err());
    }

    #[test]
    fn an_incident_maps_to_its_id_and_status_change_stamp() {
        let node = json!({
            "id": "PABCDEF",
            "incident_number": 42,
            "title": "checkout   latency",
            "status": "triggered",
            "last_status_change_at": "2026-08-08T01:00:00Z",
            "service": { "summary": "api" }
        });
        let observed = observed(&node).unwrap();
        assert_eq!(observed.key, "PABCDEF");
        assert_eq!(observed.reference(), "#42");
        assert_eq!(observed.stamp, "2026-08-08T01:00:00Z");
        assert_eq!(observed.summary, "#42 [triggered] — checkout latency · api");
    }

    #[test]
    fn an_incident_without_a_stamp_is_an_error() {
        assert!(observed(&json!({ "id": "P1" })).is_err());
    }
}

use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::REBUILD;
use goat_integration::query::{self, QueryError, ResolvedQuery, SelfRefStyle, TokenValue};
use goat_integration::shape;
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_integration_mcp::{McpService, pick_tool};
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::{AtlassianBinding, PREFIX, VOCABULARY, service};

pub const STREAM: &str = "assigned";
pub const DEFAULT_QUERY: &str = "assignee:@me is:open";

pub const SEARCH_TOOL_CANDIDATES: &[&str] = &[
    "searchJiraIssuesUsingJql",
    "search_jira_issues_using_jql",
    "searchJiraIssues",
    "jira_search",
];

const SUMMARY_LIMIT: usize = 160;
const DEFAULT_LIMIT: usize = 50;

const FIELDS: &[&str] = &["summary", "status", "updated", "assignee", "priority"];

pub fn defaults(binding: &IntegrationBinding) -> Vec<WatchSpec> {
    if cloud_id(binding).is_none() {
        return Vec::new();
    }
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
    let cloud = cloud_id(binding).ok_or_else(|| {
        IntegrationError::Config(
            "atlassian needs `cloud_id` in the agent's atlassian binding; \
             read it from the `getAccessibleAtlassianResources` tool"
                .to_owned(),
        )
    })?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Assigned,
        entity: "issue",
        diff: REBUILD,
        source: Box::new(IssueSearch {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            resolved_tool: OnceCell::new(),
            cloud,
            jql: plan.jql,
            limit: plan.limit,
        }),
    })
}

fn cloud_id(binding: &IntegrationBinding) -> Option<String> {
    AtlassianBinding::read(&binding.config).cloud_id
}

struct Plan {
    jql: String,
    limit: usize,
}

fn plan(raw: &str) -> Result<Plan, QueryError> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    Ok(Plan {
        limit: resolved.limit.unwrap_or(DEFAULT_LIMIT),
        jql: jql_of(&resolved)?,
    })
}

fn jql_of(resolved: &ResolvedQuery) -> Result<String, QueryError> {
    let mut clauses = Vec::new();
    for matched in resolved.many("assignee") {
        let op = if matched.negated { "!=" } else { "=" };
        clauses.push(match &matched.value {
            TokenValue::SelfRef => format!("assignee {op} currentUser()"),
            TokenValue::Text(name) => format!("assignee {op} {}", quoted(name)),
        });
    }
    for key in ["project", "status", "type", "priority", "label"] {
        for matched in resolved.many(key) {
            let TokenValue::Text(text) = &matched.value else {
                continue;
            };
            let field = if key == "type" { "issuetype" } else { key };
            let op = if matched.negated { "!=" } else { "=" };
            clauses.push(format!("{field} {op} {}", quoted(text)));
        }
    }
    match (resolved.state("open"), resolved.state("closed")) {
        (Some(true), Some(true)) => {
            return Err(QueryError::Invalid(
                "`is:open` and `is:closed` conflict".to_owned(),
            ));
        }
        (Some(true), _) | (_, Some(false)) => clauses.push("resolution = EMPTY".to_owned()),
        (_, Some(true)) | (Some(false), _) => clauses.push("resolution IS NOT EMPTY".to_owned()),
        (None, None) => {}
    }
    let residue = query::render(&resolved.residue, SelfRefStyle::Replace("currentUser()"));
    if !residue.trim().is_empty() {
        clauses.push(residue);
    }
    if clauses.is_empty() {
        return Err(QueryError::Invalid(
            "an atlassian watch query must say something; try `assignee:@me is:open`".to_owned(),
        ));
    }
    Ok(format!("{} ORDER BY updated DESC", clauses.join(" AND ")))
}

fn quoted(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return value.to_owned();
    }
    format!("\"{}\"", value.replace('"', "\\\""))
}

struct IssueSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    resolved_tool: OnceCell<String>,
    cloud: String,
    jql: String,
    limit: usize,
}

impl IssueSearch {
    async fn search_tool(&self, session: &goat_mcp::McpSession) -> IntegrationResult<String> {
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
            SEARCH_TOOL_CANDIDATES,
            PREFIX,
        )
        .ok_or_else(|| {
            IntegrationError::Config(format!(
                "the atlassian mcp exposes no recognized jql search tool (available: {})",
                names.join(", ")
            ))
        })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }
}

impl WatchSource for IssueSearch {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let tool = self.search_tool(&session).await;
        let result = match tool {
            Ok(tool) => {
                self.service
                    .call(
                        &session,
                        &tool,
                        json!({
                            "cloudId": self.cloud,
                            "jql": self.jql,
                            "maxResults": self.limit,
                            "fields": FIELDS,
                        }),
                    )
                    .await
            }
            Err(e) => Err(e),
        };
        session.close().await;
        let value = result?;
        let items = shape::items("atlassian", &value, &["issues"])?
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
    let key = shape::required("atlassian", node, &["key", "id"])?;
    let stamp = shape::required(
        "atlassian",
        node,
        &["fields.updated", "updated", "updatedAt"],
    )?;
    let title = shape::text(node, &["fields.summary", "summary", "title"]);
    let status = shape::text(node, &["fields.status.name", "status.name", "status"]);
    let head = shape::squeeze(&title, SUMMARY_LIMIT);
    let summary = if status.is_empty() {
        format!("{key} — {head}")
    } else {
        format!("{key} [{status}] — {head}")
    };
    Ok(Observed::new(key.clone(), stamp, summary, node.clone()).with_reference(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_default_query_becomes_current_user_jql() {
        let plan = plan(DEFAULT_QUERY).unwrap();
        assert_eq!(
            plan.jql,
            "assignee = currentUser() AND resolution = EMPTY ORDER BY updated DESC"
        );
        assert_eq!(plan.limit, DEFAULT_LIMIT);
    }

    #[test]
    fn unknown_tokens_pass_through_as_raw_jql() {
        let plan = plan("assignee:@me sprint in openSprints()").unwrap();
        assert!(plan.jql.contains("sprint in openSprints()"));
    }

    #[test]
    fn conflicting_state_is_refused() {
        assert!(plan("is:open is:closed").is_err());
        assert!(plan("limit:9999").is_err());
    }

    #[test]
    fn values_needing_quotes_get_them() {
        let plan = plan("project:\"My Team\" type:Bug").unwrap();
        assert!(plan.jql.contains("project = \"My Team\""));
        assert!(plan.jql.contains("issuetype = Bug"));
    }

    #[test]
    fn a_closed_query_inverts_the_resolution_clause() {
        assert!(plan("is:closed").unwrap().jql.contains("IS NOT EMPTY"));
    }

    #[test]
    fn an_empty_query_is_refused_rather_than_matching_everything() {
        assert!(plan("").is_err());
    }

    #[test]
    fn an_issue_maps_to_its_key_and_updated_stamp() {
        let node = json!({
            "id": "10042",
            "key": "OPS-7",
            "fields": {
                "summary": "  the   queue  backs up ",
                "updated": "2026-08-08T01:00:00.000+0000",
                "status": { "name": "In Progress" }
            }
        });
        let observed = observed(&node).unwrap();
        assert_eq!(observed.key, "OPS-7");
        assert_eq!(observed.reference(), "OPS-7");
        assert_eq!(observed.stamp, "2026-08-08T01:00:00.000+0000");
        assert_eq!(observed.summary, "OPS-7 [In Progress] — the queue backs up");
    }

    #[test]
    fn an_issue_without_an_updated_stamp_is_an_error_not_a_silent_refire() {
        assert!(observed(&json!({ "key": "OPS-7" })).is_err());
    }

    #[test]
    fn defaults_wait_for_a_cloud_id() {
        let empty = IntegrationBinding::from_config(json!({}));
        assert!(defaults(&empty).is_empty());
        let bound = IntegrationBinding::from_config(json!({ "cloud_id": "abc" }));
        assert_eq!(defaults(&bound).len(), 1);
    }
}

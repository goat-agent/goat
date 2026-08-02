use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::REBUILD;
use goat_integration::query::{self, QueryError, TokenValue};
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{IntegrationBinding, IntegrationResult, IntegrationRuntime};
use goat_integration_mcp::McpService;
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};

use crate::parse::parse_assigned_issues;
use crate::{VOCABULARY, service};

pub const STREAM: &str = "assigned";
pub const DEFAULT_QUERY: &str = "assignee:@me is:open";
pub const TOOL_LIST_ISSUES: &str = "list_issues";
pub const DEFAULT_ORDER_BY: &str = "updatedAt";
const SUMMARY_LIMIT: usize = 160;

pub fn defaults(_: &IntegrationBinding) -> Vec<WatchSpec> {
    vec![WatchSpec {
        stream: STREAM.to_owned(),
        query: DEFAULT_QUERY.to_owned(),
    }]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keep {
    All,
    Open,
    Closed,
}

#[derive(Debug)]
struct Plan {
    arguments: Value,
    keep: Keep,
    kind: IntegrationUpdateKind,
}

pub fn compile(
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    spec: &WatchSpec,
) -> IntegrationResult<CompiledWatch> {
    let plan = plan(&spec.query)?;
    Ok(CompiledWatch {
        kind: plan.kind,
        entity: "issue",
        diff: REBUILD,
        source: Box::new(IssueSearch {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            arguments: plan.arguments,
            keep: plan.keep,
        }),
    })
}

fn plan(raw: &str) -> Result<Plan, QueryError> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let keep = keep_of(&resolved)?;
    let mut arguments = json!({ "orderBy": DEFAULT_ORDER_BY });
    if let Some(limit) = resolved.limit {
        arguments["limit"] = json!(limit);
    }
    for key in ["assignee", "team", "project", "label", "state", "cycle"] {
        if let Some(found) = resolved.single(key) {
            arguments[key] = match &found.value {
                TokenValue::SelfRef => json!("me"),
                TokenValue::Text(text) => json!(text),
            };
        }
    }
    if let Some(priority) = resolved.single("priority")
        && let TokenValue::Text(name) = &priority.value
    {
        arguments["priority"] = json!(priority_number(name));
    }
    if !resolved.terms.is_empty() {
        arguments["query"] = json!(resolved.terms.join(" "));
    }
    let kind = if resolved.single("assignee").is_some() {
        IntegrationUpdateKind::Assigned
    } else {
        IntegrationUpdateKind::Updated
    };
    Ok(Plan {
        arguments,
        keep,
        kind,
    })
}

fn keep_of(resolved: &query::ResolvedQuery) -> Result<Keep, QueryError> {
    let open = resolved.state("open");
    let closed = resolved.state("closed");
    let wants_open = open == Some(true) || closed == Some(false);
    let wants_closed = closed == Some(true) || open == Some(false);
    match (wants_open, wants_closed) {
        (true, true) => Err(QueryError::Invalid(
            "`is:open` and `is:closed` conflict".to_owned(),
        )),
        (true, false) => Ok(Keep::Open),
        (false, true) => Ok(Keep::Closed),
        (false, false) => Ok(Keep::All),
    }
}

fn priority_number(name: &str) -> u8 {
    match name {
        "urgent" => 1,
        "high" => 2,
        "medium" => 3,
        "low" => 4,
        _ => 0,
    }
}

struct IssueSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    arguments: Value,
    keep: Keep,
}

impl WatchSource for IssueSearch {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let result = self
            .service
            .call(&session, TOOL_LIST_ISSUES, self.arguments.clone())
            .await;
        session.close().await;
        let value = result?;
        let issues = parse_assigned_issues(&value)?;
        Ok(WatchPage {
            items: issues
                .into_iter()
                .filter(|issue| match self.keep {
                    Keep::All => true,
                    Keep::Open => !issue.is_closed(),
                    Keep::Closed => issue.is_closed(),
                })
                .map(|issue| {
                    Observed::new(
                        issue.id.clone(),
                        issue.updated_at.clone(),
                        summary(&issue.identifier, &issue.title),
                        issue.raw,
                    )
                    .with_reference(issue.identifier)
                })
                .collect(),
            truncated: Some(crate::parse::has_next_page(&value)),
        })
    }
}

fn summary(identifier: &str, title: &str) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let clamped = if flat.chars().count() <= SUMMARY_LIMIT {
        flat
    } else {
        let kept: String = flat.chars().take(SUMMARY_LIMIT).collect();
        format!("{kept}…")
    };
    format!("{identifier} — {clamped}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_query_matches_what_was_hardcoded_before() {
        let plan = plan(DEFAULT_QUERY).unwrap();
        assert_eq!(
            plan.arguments,
            json!({ "assignee": "me", "orderBy": "updatedAt", "limit": 50 })
        );
        assert_eq!(plan.keep, Keep::Open);
        assert_eq!(plan.kind, IntegrationUpdateKind::Assigned);
    }

    #[test]
    fn a_rich_query_compiles_to_structured_arguments() {
        let plan = plan("assignee:@me is:open label:bug priority:urgent 결제 limit:25").unwrap();
        assert_eq!(
            plan.arguments,
            json!({
                "assignee": "me",
                "orderBy": "updatedAt",
                "limit": 25,
                "label": "bug",
                "priority": 1,
                "query": "결제",
            })
        );
    }

    #[test]
    fn conflicting_states_error_at_compile() {
        assert!(matches!(
            plan("is:open is:closed"),
            Err(QueryError::Invalid(_))
        ));
    }

    #[test]
    fn state_shapes_map_to_keep_policies() {
        let keep = |q: &str| plan(q).unwrap().keep;
        assert_eq!(keep("assignee:@me"), Keep::All);
        assert_eq!(keep("is:open"), Keep::Open);
        assert_eq!(keep("-is:closed"), Keep::Open);
        assert_eq!(keep("is:closed"), Keep::Closed);
        assert_eq!(keep("-is:open"), Keep::Closed);
    }

    #[test]
    fn priorities_map_by_name() {
        assert_eq!(priority_number("urgent"), 1);
        assert_eq!(priority_number("high"), 2);
        assert_eq!(priority_number("medium"), 3);
        assert_eq!(priority_number("low"), 4);
        assert_eq!(priority_number("none"), 0);
    }

    #[test]
    fn a_typo_names_the_known_keys() {
        let err = plan("asignee:@me").unwrap_err();
        let QueryError::UnknownKey { known, .. } = err else {
            panic!("expected UnknownKey");
        };
        assert_eq!(
            known,
            "assignee, cycle, is, label, limit, priority, project, state, team"
        );
    }

    #[test]
    fn a_query_without_an_assignee_briefs_as_updated() {
        assert_eq!(
            plan("team:ENG").unwrap().kind,
            IntegrationUpdateKind::Updated
        );
    }

    #[test]
    fn a_long_title_is_clamped_the_way_the_other_leaves_clamp() {
        let long = "x".repeat(400);
        let rendered = summary("US-1", &long);
        assert!(rendered.starts_with("US-1 — "));
        assert!(rendered.ends_with('…'));
        assert!(rendered.chars().count() < 200);
    }

    #[test]
    fn whitespace_in_a_title_is_flattened() {
        assert_eq!(summary("US-1", "a\n  b\tc"), "US-1 — a b c");
    }
}

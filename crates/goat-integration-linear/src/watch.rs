use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::REBUILD;
use goat_integration::watch::{Observed, Watch, WatchPage, WatchSource, run};
use goat_integration::{IntegrationBinding, IntegrationResult, IntegrationRuntime};
use goat_integration_mcp::McpService;
use goat_types::{AgentId, IntegrationUpdateKind};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::parse::parse_assigned_issues;
use crate::{LinearBinding, service};

pub const STREAM: &str = "assigned";
pub const TOOL_LIST_ISSUES: &str = "list_issues";
pub const DEFAULT_ASSIGNEE: &str = "me";
pub const DEFAULT_ORDER_BY: &str = "updatedAt";
const FETCH_LIMIT: usize = 50;
const SUMMARY_LIMIT: usize = 160;

#[allow(
    clippy::unnecessary_wraps,
    reason = "WatchFn may decline; linear always opts in"
)]
pub fn spawn(
    agent: AgentId,
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    let settings = LinearBinding::read(&binding.config);
    let source = AssignedIssues {
        service: Arc::new(service()),
        credentials: runtime.credentials.clone(),
        binding: binding.clone(),
        assignee: settings
            .assignee
            .unwrap_or_else(|| DEFAULT_ASSIGNEE.to_owned()),
        team: settings.team,
        project: settings.project,
        include_closed: settings.include_closed.unwrap_or(false),
    };
    let watch = Watch::new(
        crate::ID,
        STREAM,
        IntegrationUpdateKind::Assigned,
        "issue",
        "newly assigned",
        REBUILD,
        source,
    );
    Some(tokio::spawn(run(
        watch,
        agent,
        runtime.clone(),
        binding.account.clone(),
        cancel,
    )))
}

struct AssignedIssues {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    assignee: String,
    team: Option<String>,
    project: Option<String>,
    include_closed: bool,
}

impl AssignedIssues {
    fn arguments(&self) -> Value {
        let mut arguments = json!({
            "assignee": self.assignee,
            "orderBy": DEFAULT_ORDER_BY,
            "limit": FETCH_LIMIT,
        });
        if let Some(team) = &self.team {
            arguments["team"] = json!(team);
        }
        if let Some(project) = &self.project {
            arguments["project"] = json!(project);
        }
        arguments
    }
}

impl WatchSource for AssignedIssues {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let result = self
            .service
            .call(&session, TOOL_LIST_ISSUES, self.arguments())
            .await;
        session.close().await;
        let value = result?;
        let issues = parse_assigned_issues(&value)?;
        Ok(WatchPage {
            items: issues
                .into_iter()
                .filter(|issue| self.include_closed || !issue.is_closed())
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

    fn source(settings: LinearBinding) -> AssignedIssues {
        AssignedIssues {
            service: Arc::new(service()),
            credentials: CredentialStore::new(std::path::PathBuf::from("/tmp/unused.json")),
            binding: IntegrationBinding::from_config(json!({})),
            assignee: settings
                .assignee
                .unwrap_or_else(|| DEFAULT_ASSIGNEE.to_owned()),
            team: settings.team,
            project: settings.project,
            include_closed: settings.include_closed.unwrap_or(false),
        }
    }

    #[test]
    fn the_default_query_matches_what_was_hardcoded_before() {
        let arguments = source(LinearBinding::default()).arguments();
        assert_eq!(arguments["assignee"], "me");
        assert_eq!(arguments["orderBy"], "updatedAt");
        assert_eq!(arguments["limit"], 50);
        assert!(arguments.get("team").is_none());
        assert!(arguments.get("project").is_none());
    }

    #[test]
    fn the_owner_can_watch_someone_elses_queue_or_narrow_by_team() {
        let arguments = source(LinearBinding {
            assignee: Some("jmo".to_owned()),
            team: Some("ENG".to_owned()),
            project: Some("Platform".to_owned()),
            include_closed: None,
        })
        .arguments();
        assert_eq!(arguments["assignee"], "jmo");
        assert_eq!(arguments["team"], "ENG");
        assert_eq!(arguments["project"], "Platform");
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

use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::RETAIN;
use goat_integration::watch::{Observed, Watch, WatchPage, WatchSource, run};
use goat_integration::{IntegrationBinding, IntegrationResult, IntegrationRuntime};
use goat_integration_mcp::McpService;
use goat_types::{AgentId, IntegrationUpdateKind};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::parse::parse_issues;
use crate::{SentryBinding, service};

pub const STREAM: &str = "issues";
pub const TOOL_SEARCH_ISSUES: &str = "search_issues";
pub const DEFAULT_QUERY: &str = "is:unresolved is:for_review";
pub const DEFAULT_SORT: &str = "new";

pub fn spawn(
    agent: AgentId,
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    let settings = SentryBinding::read(&binding.config);
    let Some(organization_slug) = settings.organization_slug else {
        warn!(
            agent = %agent,
            "sentry watcher disabled; set `organization_slug` in the agent's sentry binding",
        );
        return None;
    };
    let source = IssueSearch {
        service: Arc::new(service()),
        credentials: runtime.credentials.clone(),
        binding: binding.clone(),
        organization_slug,
        project: settings.project,
        query: settings.query.unwrap_or_else(|| DEFAULT_QUERY.to_owned()),
        sort: settings.sort.unwrap_or_else(|| DEFAULT_SORT.to_owned()),
    };
    let watch = Watch::new(
        crate::ID,
        STREAM,
        IntegrationUpdateKind::Updated,
        "issue",
        "issues waiting",
        RETAIN,
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

struct IssueSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    organization_slug: String,
    project: Option<String>,
    query: String,
    sort: String,
}

impl IssueSearch {
    fn arguments(&self) -> Value {
        let mut arguments = json!({
            "organizationSlug": self.organization_slug,
            "query": self.query,
            "sort": self.sort,
        });
        if let Some(project) = &self.project {
            arguments["projectSlugOrId"] = json!(project);
        }
        arguments
    }
}

impl WatchSource for IssueSearch {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let result = self
            .service
            .call(&session, TOOL_SEARCH_ISSUES, self.arguments())
            .await;
        session.close().await;
        let issues = parse_issues(&result?)?;
        Ok(WatchPage::new(
            issues
                .into_iter()
                .map(|issue| {
                    Observed::new(
                        issue.key.clone(),
                        issue.last_seen.clone(),
                        issue.summary(),
                        issue.raw,
                    )
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(settings: SentryBinding) -> IssueSearch {
        IssueSearch {
            service: Arc::new(service()),
            credentials: CredentialStore::new(std::path::PathBuf::from("/tmp/unused.json")),
            binding: IntegrationBinding::from_config(json!({})),
            organization_slug: settings.organization_slug.unwrap_or_default(),
            project: settings.project,
            query: settings.query.unwrap_or_else(|| DEFAULT_QUERY.to_owned()),
            sort: settings.sort.unwrap_or_else(|| DEFAULT_SORT.to_owned()),
        }
    }

    #[test]
    fn the_defaults_match_what_the_watcher_used_before() {
        let arguments = source(SentryBinding {
            organization_slug: Some("acme".to_owned()),
            ..SentryBinding::default()
        })
        .arguments();
        assert_eq!(arguments["organizationSlug"], "acme");
        assert_eq!(arguments["query"], "is:unresolved is:for_review");
        assert_eq!(arguments["sort"], "new");
        assert!(arguments.get("projectSlugOrId").is_none());
    }

    #[test]
    fn a_project_narrows_the_search_when_given() {
        let arguments = source(SentryBinding {
            organization_slug: Some("acme".to_owned()),
            project: Some("backend".to_owned()),
            query: Some("is:unresolved".to_owned()),
            sort: Some("freq".to_owned()),
        })
        .arguments();
        assert_eq!(arguments["projectSlugOrId"], "backend");
        assert_eq!(arguments["query"], "is:unresolved");
        assert_eq!(arguments["sort"], "freq");
    }
}

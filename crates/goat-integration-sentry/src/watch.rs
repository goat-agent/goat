use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_integration::diff::RETAIN;
use goat_integration::query::{self, SelfRefStyle, TokenValue};
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_integration_mcp::McpService;
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};

use crate::parse::parse_issues;
use crate::{SentryBinding, VOCABULARY, service};

pub const STREAM: &str = "issues";
pub const TOOL_SEARCH_ISSUES: &str = "search_issues";
pub const DEFAULT_QUERY: &str = "is:unresolved is:for_review sort:new";

pub fn defaults(binding: &IntegrationBinding) -> Vec<WatchSpec> {
    if SentryBinding::read(&binding.config)
        .organization_slug
        .is_none()
    {
        return Vec::new();
    }
    vec![WatchSpec {
        stream: STREAM.to_owned(),
        query: DEFAULT_QUERY.to_owned(),
    }]
}

pub fn compile(
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    spec: &WatchSpec,
) -> IntegrationResult<CompiledWatch> {
    let arguments = plan(&binding.config, &spec.query)?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Updated,
        entity: "issue",
        diff: RETAIN,
        source: Box::new(IssueSearch {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            arguments,
        }),
    })
}

fn plan(config: &Value, raw: &str) -> IntegrationResult<Value> {
    let Some(organization_slug) = SentryBinding::read(config).organization_slug else {
        return Err(IntegrationError::Config(
            "sentry watch needs `organization_slug` in the agent's sentry binding".into(),
        ));
    };
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let mut arguments = json!({
        "organizationSlug": organization_slug,
        "query": query::render(&resolved.residue, SelfRefStyle::Replace("me")),
    });
    if let Some(sort) = resolved.single("sort")
        && let TokenValue::Text(sort) = &sort.value
    {
        arguments["sort"] = json!(sort);
    }
    if let Some(project) = resolved.single("project")
        && let TokenValue::Text(project) = &project.value
    {
        arguments["projectSlugOrId"] = json!(project);
    }
    Ok(arguments)
}

struct IssueSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    arguments: Value,
}

impl WatchSource for IssueSearch {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let result = self
            .service
            .call(&session, TOOL_SEARCH_ISSUES, self.arguments.clone())
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
    use goat_integration::query::QueryError;

    #[test]
    fn the_default_stream_compiles_to_the_request_the_watcher_sent_before() {
        let arguments = plan(&json!({ "organization_slug": "acme" }), DEFAULT_QUERY).unwrap();
        assert_eq!(
            arguments,
            json!({
                "organizationSlug": "acme",
                "query": "is:unresolved is:for_review",
                "sort": "new",
            })
        );
    }

    #[test]
    fn defaults_decline_until_the_organization_is_bound() {
        let bare = IntegrationBinding::from_config(json!({}));
        assert!(defaults(&bare).is_empty());
        let bound = IntegrationBinding::from_config(json!({ "organization_slug": "acme" }));
        assert_eq!(
            defaults(&bound),
            vec![WatchSpec {
                stream: STREAM.to_owned(),
                query: DEFAULT_QUERY.to_owned(),
            }]
        );
    }

    #[test]
    fn a_missing_organization_fails_the_compile_loudly() {
        let err = plan(&json!({}), DEFAULT_QUERY).unwrap_err();
        assert!(matches!(err, IntegrationError::Config(_)));
        assert!(err.to_string().contains("organization_slug"));
    }

    #[test]
    fn project_and_sort_map_to_their_request_arguments() {
        let arguments = plan(
            &json!({ "organization_slug": "acme" }),
            "is:unresolved project:backend sort:freq",
        )
        .unwrap();
        assert_eq!(
            arguments,
            json!({
                "organizationSlug": "acme",
                "query": "is:unresolved",
                "sort": "freq",
                "projectSlugOrId": "backend",
            })
        );
    }

    #[test]
    fn native_tokens_pass_through_verbatim_and_selfrefs_become_me() {
        let arguments = plan(
            &json!({ "organization_slug": "acme" }),
            "assigned:@me is:unresolved level:error \"payment failed\"",
        )
        .unwrap();
        assert_eq!(
            arguments["query"],
            "assigned:me is:unresolved level:error \"payment failed\""
        );
        assert!(arguments.get("sort").is_none());
        assert!(arguments.get("projectSlugOrId").is_none());
    }

    #[test]
    fn repeated_dsl_keys_error_at_compile() {
        let err = plan(
            &json!({ "organization_slug": "acme" }),
            "sort:new sort:freq",
        )
        .unwrap_err();
        assert!(err.to_string().contains("once"));
        let err = plan(&json!({ "organization_slug": "acme" }), "limit:5").unwrap_err();
        assert!(err.to_string().contains("limit"));
    }

    #[test]
    fn broken_queries_surface_as_config_errors() {
        let err = plan(&json!({ "organization_slug": "acme" }), "sort:").unwrap_err();
        assert!(matches!(err, IntegrationError::Config(_)));
        assert_eq!(
            query::parse("sort:").unwrap_err(),
            QueryError::DanglingKey("sort".to_owned())
        );
    }
}

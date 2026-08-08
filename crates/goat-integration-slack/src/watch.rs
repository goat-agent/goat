use std::sync::{Arc, OnceLock};

use goat_auth::CredentialStore;
use goat_integration::diff::RETAIN;
use goat_integration::query::{self, SelfRefStyle};
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_integration_mcp::{McpService, pick_tool};
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};
use tracing::warn;

use crate::parse::parse_mentions;
use crate::{PREFIX, SlackBinding, VOCABULARY, service};

pub const STREAM: &str = "mentions";
pub const DEFAULT_QUERY: &str = "@me";

const SEARCH_TOOL_CANDIDATES: &[&str] = &[
    "search_public_and_private",
    "search_messages",
    "search_public",
];

pub fn defaults(binding: &IntegrationBinding) -> Vec<WatchSpec> {
    if SlackBinding::read(&binding.config).user_id.is_none() {
        warn!(
            "slack watcher disabled; set `user_id` to your Slack member ID in the agent's slack binding",
        );
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
    let settings = SlackBinding::read(&binding.config);
    let plan = plan(&spec.query, settings.user_id.as_deref())?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Updated,
        entity: "message",
        diff: RETAIN,
        source: Box::new(MentionSearch {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            arguments: plan.arguments,
            configured_tool: settings.search_tool,
            resolved_tool: OnceLock::new(),
            self_id: plan.self_id,
        }),
    })
}

#[derive(Debug)]
struct Plan {
    arguments: Value,
    self_id: String,
}

fn plan(raw: &str, user_id: Option<&str>) -> IntegrationResult<Plan> {
    let Some(user_id) = user_id else {
        return Err(IntegrationError::Config(
            "slack watch needs `user_id`; set it to your Slack member ID in the agent's slack binding"
                .into(),
        ));
    };
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let mention = format!("<@{user_id}>");
    let mut arguments = json!({
        "query": query::render(&resolved.residue, SelfRefStyle::Replace(&mention)),
    });
    if let Some(limit) = resolved.limit {
        arguments["limit"] = json!(limit);
    }
    Ok(Plan {
        arguments,
        self_id: user_id.to_owned(),
    })
}

struct MentionSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    arguments: Value,
    configured_tool: Option<String>,
    resolved_tool: OnceLock<String>,
    self_id: String,
}

impl MentionSearch {
    async fn search_tool(&self, session: &goat_mcp::McpSession) -> IntegrationResult<String> {
        if let Some(tool) = self.resolved_tool.get() {
            return Ok(tool.clone());
        }
        if let Some(configured) = &self.configured_tool {
            let _ = self.resolved_tool.set(configured.clone());
            return Ok(configured.clone());
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
                "slack mcp exposes no recognized search tool; set `search_tool` in the agent's slack binding (available: {})",
                names.join(", ")
            ))
        })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }
}

impl WatchSource for MentionSearch {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let tool = match self.search_tool(&session).await {
            Ok(tool) => tool,
            Err(e) => {
                session.close().await;
                return Err(e);
            }
        };
        let result = self
            .service
            .call(&session, &tool, self.arguments.clone())
            .await;
        session.close().await;
        let mentions = parse_mentions(&result?)?;
        Ok(WatchPage::new(
            mentions
                .into_iter()
                .filter(|mention| !mention.is_authored_by(&self.self_id))
                .map(|mention| {
                    Observed::new(
                        mention.key.clone(),
                        mention.ts.clone(),
                        mention.summary(),
                        mention.raw,
                    )
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_query_matches_what_was_hardcoded_before() {
        let plan = plan(DEFAULT_QUERY, Some("U0OWNER")).unwrap();
        assert_eq!(plan.arguments["query"], "<@U0OWNER>");
        assert_eq!(
            plan.arguments,
            json!({ "query": "<@U0OWNER>", "limit": 50 })
        );
        assert_eq!(plan.self_id, "U0OWNER");
    }

    #[test]
    fn slack_modifiers_pass_through_verbatim() {
        let plan = plan(
            r#"@me in:#eng from:@alice has:link before:2026-01-01 "deploy failed" limit:25"#,
            Some("U1"),
        )
        .unwrap();
        assert_eq!(
            plan.arguments,
            json!({
                "query": r#"<@U1> in:#eng from:@alice has:link before:2026-01-01 "deploy failed""#,
                "limit": 25,
            })
        );
    }

    #[test]
    fn compiling_without_a_member_id_points_at_the_binding() {
        let err = plan(DEFAULT_QUERY, None).unwrap_err();
        assert!(err.to_string().contains("`user_id`"));
        assert!(err.to_string().contains("slack binding"));
    }

    #[test]
    fn a_broken_query_surfaces_the_dsl_error() {
        assert!(plan(r#"in:"eng"#, Some("U1")).is_err());
        assert!(plan("limit:0", Some("U1")).is_err());
        assert!(plan("limit:9999", Some("U1")).is_err());
    }

    #[test]
    fn defaults_decline_without_a_member_id() {
        let empty = IntegrationBinding::from_config(json!({}));
        assert!(defaults(&empty).is_empty());
        let bound = IntegrationBinding::from_config(json!({ "user_id": "U1" }));
        assert_eq!(
            defaults(&bound),
            vec![WatchSpec {
                state_key: STREAM.to_owned(),
                stream: STREAM.to_owned(),
                query: DEFAULT_QUERY.to_owned(),
            }]
        );
    }

    #[test]
    fn the_search_tool_is_picked_from_what_the_server_exposes() {
        let available = ["slack_search_public", "slack_send_message"];
        let picked = pick_tool(available.iter().copied(), SEARCH_TOOL_CANDIDATES, PREFIX);
        assert_eq!(picked.as_deref(), Some("slack_search_public"));
    }

    #[test]
    fn a_doubled_prefix_is_not_mistaken_for_the_search_tool() {
        let available = ["slack_slack_search_public"];
        assert!(pick_tool(available.iter().copied(), SEARCH_TOOL_CANDIDATES, PREFIX).is_none());
    }

    #[test]
    fn a_hyphenated_server_spelling_still_resolves() {
        let available = ["slack-search-public"];
        let picked = pick_tool(available.iter().copied(), SEARCH_TOOL_CANDIDATES, PREFIX);
        assert_eq!(picked.as_deref(), Some("slack-search-public"));
    }
}

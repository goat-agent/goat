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

use crate::{PREFIX, VOCABULARY, VercelBinding, service};

#[cfg(test)]
pub const STREAM: &str = "deploys";

pub const LIST_TOOL_CANDIDATES: &[&str] = &[
    "list_deployments",
    "get_deployments",
    "deployments_list",
    "list-deployments",
];

const SUMMARY_LIMIT: usize = 160;

pub fn defaults(_: &IntegrationBinding) -> Vec<WatchSpec> {
    Vec::new()
}

pub fn compile(
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    spec: &WatchSpec,
) -> IntegrationResult<CompiledWatch> {
    let team = VercelBinding::read(&binding.config).team.ok_or_else(|| {
        IntegrationError::Config(
            "vercel needs `team` in the agent's vercel binding; \
             the `vercel_list_teams` tool prints the id"
                .to_owned(),
        )
    })?;
    let plan = plan(&spec.query, &team)?;
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Updated,
        entity: "deployment",
        diff: SETTLE,
        source: Box::new(DeploymentSearch {
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

fn plan(raw: &str, team: &str) -> IntegrationResult<Plan> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let project = resolved
        .single("project")
        .and_then(|matched| match &matched.value {
            TokenValue::Text(text) => Some(text.clone()),
            TokenValue::SelfRef => None,
        })
        .ok_or_else(|| {
            IntegrationError::Config(
                "a vercel watch query must name a project, as in `project:goat-web state:error`; \
                 the deployment list tool takes no wildcard"
                    .to_owned(),
            )
        })?;
    let states: Vec<String> = resolved
        .many("state")
        .filter(|m| !m.negated)
        .filter_map(|m| match &m.value {
            TokenValue::Text(text) => Some(text.to_lowercase()),
            TokenValue::SelfRef => None,
        })
        .collect();
    Ok(Plan {
        arguments: json!({ "projectId": project, "teamId": team }),
        states,
    })
}

struct DeploymentSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    resolved_tool: OnceCell<String>,
    arguments: Value,
    states: Vec<String>,
}

impl DeploymentSearch {
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
                "the vercel mcp exposes no recognized deployment list tool (available: {})",
                names.join(", ")
            ))
        })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }

    fn keeps(&self, node: &Value) -> bool {
        if self.states.is_empty() {
            return true;
        }
        let state = state_of(node);
        self.states.iter().any(|wanted| wanted == &state)
    }
}

impl WatchSource for DeploymentSearch {
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
        let items = shape::items("vercel", &value, &["deployments"])?
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

fn state_of(node: &Value) -> String {
    shape::text(node, &["state", "readyState", "status"]).to_lowercase()
}

fn observed(node: &Value) -> IntegrationResult<Observed> {
    let key = shape::required("vercel", node, &["uid", "id", "url"])?;
    let state = state_of(node);
    let created = shape::text(node, &["createdAt", "created", "ready", "buildingAt"]);
    let name = shape::text(node, &["name", "meta.githubCommitRepo"]);
    let url = shape::text(node, &["url"]);
    let message = shape::squeeze(
        &shape::text(node, &["meta.githubCommitMessage", "meta.gitCommitMessage"]),
        SUMMARY_LIMIT,
    );
    let reference = if url.is_empty() { key.clone() } else { url };
    let head = if name.is_empty() {
        reference.clone()
    } else {
        name
    };
    let summary = if message.is_empty() {
        format!("{head} [{state}]")
    } else {
        format!("{head} [{state}] — {message}")
    };
    Ok(
        Observed::new(key, format!("{state}:{created}"), summary, node.clone())
            .with_reference(reference),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_query_without_a_project_is_refused_because_the_tool_needs_one() {
        assert!(plan("state:error", "team_1").is_err());
        let plan = plan("project:web state:error", "team_1").unwrap();
        assert_eq!(plan.arguments["projectId"], json!("web"));
        assert_eq!(plan.arguments["teamId"], json!("team_1"));
        assert_eq!(plan.states, vec!["error".to_owned()]);
    }

    #[test]
    fn there_is_no_default_watch_since_a_project_cannot_be_guessed() {
        assert!(defaults(&IntegrationBinding::from_config(json!({}))).is_empty());
    }

    #[test]
    fn unknown_keys_and_free_text_are_refused() {
        assert!(plan("project:web branch:main", "t").is_err());
        assert!(plan("project:web words", "t").is_err());
        assert!(plan("project:web state:nonsense", "t").is_err());
    }

    #[test]
    fn the_state_filter_runs_client_side_because_the_tool_has_no_state_argument() {
        let search = DeploymentSearch {
            service: Arc::new(service()),
            credentials: CredentialStore::new(std::path::PathBuf::from("/dev/null")),
            binding: IntegrationBinding::from_config(json!({})),
            resolved_tool: OnceCell::new(),
            arguments: json!({}),
            states: vec!["error".to_owned()],
        };
        assert!(search.keeps(&json!({ "uid": "a", "readyState": "ERROR" })));
        assert!(!search.keeps(&json!({ "uid": "b", "readyState": "READY" })));
    }

    #[test]
    fn the_stamp_carries_the_state_so_a_build_that_fails_re_fires() {
        let building = observed(&json!({
            "uid": "dpl_1",
            "name": "goat-web",
            "url": "goat-web-abc.vercel.app",
            "readyState": "BUILDING",
            "createdAt": 1_754_600_000_000_u64,
            "meta": { "githubCommitMessage": "fix   the   thing" }
        }))
        .unwrap();
        let failed = observed(&json!({
            "uid": "dpl_1",
            "name": "goat-web",
            "url": "goat-web-abc.vercel.app",
            "readyState": "ERROR",
            "createdAt": 1_754_600_000_000_u64,
            "meta": { "githubCommitMessage": "fix the thing" }
        }))
        .unwrap();
        assert_eq!(building.key, failed.key);
        assert_ne!(building.stamp, failed.stamp);
        assert_eq!(failed.reference(), "goat-web-abc.vercel.app");
        assert_eq!(failed.summary, "goat-web [error] — fix the thing");
    }

    #[test]
    fn a_deployment_without_an_id_is_an_error() {
        assert!(observed(&json!({ "name": "x" })).is_err());
    }
}

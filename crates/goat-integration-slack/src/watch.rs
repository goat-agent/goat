use std::sync::{Arc, OnceLock};

use goat_auth::CredentialStore;
use goat_integration::diff::RETAIN;
use goat_integration::watch::{Observed, Watch, WatchPage, WatchSource, run};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_integration_mcp::{McpService, pick_tool};
use goat_types::{IntegrationUpdateKind, ProfileId};
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::parse::parse_mentions;
use crate::{PREFIX, SlackBinding, service};

pub const STREAM: &str = "mentions";
const FETCH_LIMIT: usize = 50;

const SEARCH_TOOL_CANDIDATES: &[&str] = &[
    "search_public_and_private",
    "search_messages",
    "search_public",
];

pub fn spawn(
    persona: ProfileId,
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    let settings = SlackBinding::read(&binding.config);
    let Some(user_id) = settings.user_id else {
        warn!(
            profile = %persona,
            "slack watcher disabled; set `user_id` to your Slack member ID in the agent's slack binding",
        );
        return None;
    };
    let source = MentionSearch {
        service: Arc::new(service()),
        credentials: runtime.credentials.clone(),
        binding: binding.clone(),
        query: settings.query.unwrap_or_else(|| format!("<@{user_id}>")),
        configured_tool: settings.search_tool,
        resolved_tool: OnceLock::new(),
        self_id: user_id,
    };
    let watch = Watch::new(
        crate::ID,
        STREAM,
        IntegrationUpdateKind::Updated,
        "message",
        "waiting on you",
        RETAIN,
        source,
    );
    Some(tokio::spawn(run(
        watch,
        persona,
        runtime.clone(),
        binding.account.clone(),
        cancel,
    )))
}

struct MentionSearch {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    query: String,
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
            .call(
                &session,
                &tool,
                json!({ "query": self.query, "limit": FETCH_LIMIT }),
            )
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

use std::sync::{Arc, OnceLock};

use goat_auth::CredentialStore;
use goat_integration::diff::REBUILD;
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

use crate::parse::{has_more, parse_rows};
use crate::{NotionBinding, PREFIX, service};

pub const STREAM: &str = "view";
const FETCH_LIMIT: usize = 50;

const VIEW_TOOL_CANDIDATES: &[&str] = &["query_data_sources", "query_database_view"];

pub fn spawn(
    persona: ProfileId,
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    let settings = NotionBinding::read(&binding.config);
    let Some(view_url) = settings.view_url else {
        warn!(
            profile = %persona,
            "notion watcher disabled; set `view_url` to a saved Notion view in the agent's notion binding",
        );
        return None;
    };
    let source = ViewRows {
        service: Arc::new(service()),
        credentials: runtime.credentials.clone(),
        binding: binding.clone(),
        view_url,
        configured_tool: settings.query_tool,
        resolved_tool: OnceLock::new(),
    };
    let watch = Watch::new(
        crate::ID,
        STREAM,
        IntegrationUpdateKind::Assigned,
        "page",
        "in the view",
        REBUILD,
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

struct ViewRows {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    view_url: String,
    configured_tool: Option<String>,
    resolved_tool: OnceLock<String>,
}

impl ViewRows {
    async fn query_tool(&self, session: &goat_mcp::McpSession) -> IntegrationResult<String> {
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
        let picked = pick_tool(names.iter().map(String::as_str), VIEW_TOOL_CANDIDATES, PREFIX)
            .ok_or_else(|| {
                IntegrationError::Config(format!(
                    "notion mcp exposes no database query tool; set `query_tool` in the agent's notion binding (available: {})",
                    names.join(", ")
                ))
            })?;
        let _ = self.resolved_tool.set(picked.clone());
        Ok(picked)
    }
}

impl WatchSource for ViewRows {
    async fn fetch(&self) -> IntegrationResult<WatchPage> {
        let session = self
            .service
            .connect(&self.credentials, &self.binding)
            .await?;
        let tool = match self.query_tool(&session).await {
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
                json!({
                    "data": {
                        "mode": "view",
                        "view_url": self.view_url,
                        "page_size": FETCH_LIMIT,
                    }
                }),
            )
            .await;
        session.close().await;
        let value = result?;
        let rows = parse_rows(&value)?;
        Ok(WatchPage {
            items: rows
                .into_iter()
                .map(|row| {
                    Observed::new(
                        row.id.clone(),
                        row.edited_at.clone(),
                        row.summary(),
                        row.raw,
                    )
                })
                .collect(),
            truncated: Some(has_more(&value)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_tool_is_found_across_naming_styles() {
        for available in [
            vec!["notion-query-data-sources"],
            vec!["notion_query_data_sources"],
            vec!["query_data_sources"],
        ] {
            let picked = pick_tool(available.iter().copied(), VIEW_TOOL_CANDIDATES, PREFIX);
            assert_eq!(picked.as_deref(), Some(available[0]));
        }
    }

    #[test]
    fn the_first_candidate_wins_over_the_fallback() {
        let available = ["notion_query_database_view", "notion_query_data_sources"];
        let picked = pick_tool(available.iter().copied(), VIEW_TOOL_CANDIDATES, PREFIX);
        assert_eq!(picked.as_deref(), Some("notion_query_data_sources"));
    }

    #[test]
    fn an_unrelated_tool_is_not_mistaken_for_the_query_tool() {
        assert!(
            pick_tool(
                ["notion-fetch"].iter().copied(),
                VIEW_TOOL_CANDIDATES,
                PREFIX
            )
            .is_none()
        );
    }
}

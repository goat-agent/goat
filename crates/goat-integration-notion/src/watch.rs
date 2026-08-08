use std::sync::{Arc, OnceLock};

use goat_auth::CredentialStore;
use goat_integration::diff::REBUILD;
use goat_integration::query::{self, QueryError, TokenValue};
use goat_integration::watch::{CompiledWatch, Observed, WatchPage, WatchSource, WatchSpec};
use goat_integration::{
    IntegrationBinding, IntegrationError, IntegrationResult, IntegrationRuntime,
};
use goat_integration_mcp::{McpService, pick_tool};
use goat_types::IntegrationUpdateKind;
use serde_json::{Value, json};

use crate::parse::{has_more, parse_rows};
use crate::{NotionBinding, PREFIX, VOCABULARY, service};

pub const FETCH_LIMIT: usize = 50;

const VIEW_TOOL_CANDIDATES: &[&str] = &["query_data_sources", "query_database_view"];

pub fn compile(
    binding: &IntegrationBinding,
    runtime: &IntegrationRuntime,
    spec: &WatchSpec,
) -> IntegrationResult<CompiledWatch> {
    let arguments = plan(&spec.query)?;
    let settings = NotionBinding::read(&binding.config);
    Ok(CompiledWatch {
        kind: IntegrationUpdateKind::Assigned,
        entity: "page",
        diff: REBUILD,
        source: Box::new(ViewRows {
            service: Arc::new(service()),
            credentials: runtime.credentials.clone(),
            binding: binding.clone(),
            arguments,
            configured_tool: settings.query_tool,
            resolved_tool: OnceLock::new(),
        }),
    })
}

fn plan(raw: &str) -> Result<Value, QueryError> {
    let resolved = query::resolve(&VOCABULARY, query::parse(raw)?)?;
    let view = resolved
        .single("view")
        .and_then(|found| match &found.value {
            TokenValue::Text(url) => Some(url.trim().to_owned()),
            TokenValue::SelfRef => None,
        })
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            QueryError::Invalid(
                "notion needs exactly one `view:<url>` — a saved Notion view URL, the one with ?v="
                    .to_owned(),
            )
        })?;
    Ok(json!({
        "data": {
            "mode": "view",
            "view_url": view,
            "page_size": resolved.limit.unwrap_or(FETCH_LIMIT),
        }
    }))
}

struct ViewRows {
    service: Arc<McpService>,
    credentials: CredentialStore,
    binding: IntegrationBinding,
    arguments: Value,
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
            .call(&session, &tool, self.arguments.clone())
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
    fn a_view_url_with_colons_compiles_to_the_old_request() {
        let arguments = plan("view:https://www.notion.so/x?v=1").unwrap();
        assert_eq!(
            arguments,
            json!({
                "data": {
                    "mode": "view",
                    "view_url": "https://www.notion.so/x?v=1",
                    "page_size": 50,
                }
            })
        );
    }

    #[test]
    fn an_explicit_limit_becomes_the_page_size() {
        let arguments = plan("view:https://www.notion.so/x?v=1 limit:25").unwrap();
        assert_eq!(arguments["data"]["page_size"], 25);
    }

    #[test]
    fn a_query_without_a_view_errors_helpfully() {
        for raw in ["", "limit:10"] {
            let QueryError::Invalid(message) = plan(raw).unwrap_err() else {
                panic!("expected Invalid");
            };
            assert!(message.contains("view:<url>"));
        }
    }

    #[test]
    fn an_empty_view_value_counts_as_missing() {
        assert!(matches!(plan(r#"view:"""#), Err(QueryError::Invalid(_))));
    }

    #[test]
    fn a_negated_view_errors() {
        assert!(matches!(
            plan("-view:https://www.notion.so/x?v=1"),
            Err(QueryError::NotNegatable(_))
        ));
    }

    #[test]
    fn a_repeated_view_errors() {
        assert!(matches!(
            plan("view:https://a view:https://b"),
            Err(QueryError::Repeated(_))
        ));
    }

    #[test]
    fn free_text_is_rejected() {
        assert!(matches!(
            plan("view:https://www.notion.so/x?v=1 stray"),
            Err(QueryError::FreeText { .. })
        ));
    }

    #[test]
    fn a_typo_names_the_known_keys() {
        let QueryError::UnknownKey { known, .. } = plan("vieww:https://x").unwrap_err() else {
            panic!("expected UnknownKey");
        };
        assert_eq!(known, "limit, view");
    }

    #[tokio::test]
    async fn compile_builds_a_page_watch_from_a_valid_spec() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = goat_integration::test_support::runtime_in(dir.path()).await;
        let binding = IntegrationBinding::from_config(json!({ "query_tool": "custom_query" }));
        let spec = WatchSpec {
            state_key: crate::STREAM.to_owned(),
            stream: crate::STREAM.to_owned(),
            query: "view:https://www.notion.so/x?v=1".to_owned(),
        };
        let compiled = compile(&binding, &runtime, &spec).unwrap();
        assert_eq!(compiled.kind, IntegrationUpdateKind::Assigned);
        assert_eq!(compiled.entity, "page");
    }

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

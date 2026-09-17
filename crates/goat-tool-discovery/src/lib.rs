use std::sync::{Arc, Mutex, PoisonError};

use goat_provider::ToolDefinition;
use goat_tool::{Tool, ToolFuture, ToolOutput, ToolSandbox};
use serde_json::Value;

pub const NAME: &str = "ToolSearch";

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;

pub type Discovered = Arc<Mutex<std::collections::HashSet<String>>>;

pub fn discovered_set() -> Discovered {
    Arc::new(Mutex::new(std::collections::HashSet::new()))
}

pub struct ToolSearchTool {
    catalog: Vec<ToolDefinition>,
    discovered: Discovered,
}

impl ToolSearchTool {
    pub fn new(catalog: Vec<ToolDefinition>, discovered: Discovered) -> Self {
        Self {
            catalog,
            discovered,
        }
    }
}

impl Tool for ToolSearchTool {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Search the deferred tool catalog for tools that are not loaded upfront. Matching tools are loaded immediately: their full definitions are returned here and become callable for the rest of the session. Use this when a task needs a capability the loaded tools do not cover."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Space-separated terms; a tool matches when every term appears in its name or description."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of tools to load (default 5, max 20).",
                    "default": DEFAULT_LIMIT
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Value = if input.trim().is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(input)?
            };
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_LIMIT, |raw| {
                    usize::try_from(raw).unwrap_or(MAX_LIMIT)
                })
                .clamp(1, MAX_LIMIT);
            let matches = search(&self.catalog, query, limit);
            {
                let mut discovered = self
                    .discovered
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                for def in &matches {
                    discovered.insert(def.name.clone());
                }
            }
            let body = serde_json::to_string_pretty(
                &matches
                    .iter()
                    .map(|def| {
                        serde_json::json!({
                            "name": def.name,
                            "description": def.description,
                            "input_schema": def.input_schema,
                        })
                    })
                    .collect::<Vec<_>>(),
            )?;
            let summary = if matches.is_empty() {
                "no matching tools".to_owned()
            } else {
                format!(
                    "loaded: {}",
                    matches
                        .iter()
                        .map(|def| def.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            Ok(ToolOutput::text(body).with_summary(summary))
        })
    }
}

fn search<'a>(catalog: &'a [ToolDefinition], query: &str, limit: usize) -> Vec<&'a ToolDefinition> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &ToolDefinition)> = catalog
        .iter()
        .filter_map(|def| {
            let name = def.name.to_lowercase();
            let haystack = format!("{} {}", name, def.description.to_lowercase());
            if !terms.iter().all(|term| haystack.contains(term.as_str())) {
                return None;
            }
            let name_hits = terms
                .iter()
                .filter(|term| name.contains(term.as_str()))
                .count();
            Some((name_hits, def))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, def)| def).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            description: description.to_owned(),
            input_schema: serde_json::json!({ "type": "object" }),
            defer_loading: true,
        }
    }

    fn catalog() -> Vec<ToolDefinition> {
        vec![
            def(
                "posthog_query",
                "Run a HogQL query against PostHog analytics",
            ),
            def("posthog_dashboard_create", "Create a PostHog dashboard"),
            def("langfuse_trace_get", "Fetch a Langfuse trace by id"),
            def("slack_send_message", "Send a Slack message to a channel"),
        ]
    }

    #[test]
    fn every_term_must_match() {
        let catalog = catalog();
        let found = search(&catalog, "posthog query", 5);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "posthog_query");
    }

    #[test]
    fn name_hits_rank_before_description_hits() {
        let catalog = catalog();
        let found = search(&catalog, "posthog", 5);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "posthog_dashboard_create");
        assert_eq!(found[1].name, "posthog_query");
    }

    #[test]
    fn limit_caps_the_result() {
        let catalog = catalog();
        let found = search(&catalog, "posthog", 1);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn empty_query_matches_nothing() {
        let catalog = catalog();
        assert!(search(&catalog, "   ", 5).is_empty());
    }

    #[tokio::test]
    async fn run_loads_matches_into_the_discovered_set() {
        let discovered = discovered_set();
        let tool = ToolSearchTool::new(catalog(), discovered.clone());
        let ctx = ToolSandbox::new(std::env::temp_dir().as_path()).unwrap();
        let out = tool
            .run("{\"query\":\"langfuse\"}", &ctx)
            .await
            .expect("search runs");
        let text = out.as_text().unwrap();
        assert!(text.contains("langfuse_trace_get"));
        assert!(text.contains("input_schema"));
        assert!(discovered.lock().unwrap().contains("langfuse_trace_get"));
    }

    #[tokio::test]
    async fn a_miss_loads_nothing() {
        let discovered = discovered_set();
        let tool = ToolSearchTool::new(catalog(), discovered.clone());
        let ctx = ToolSandbox::new(std::env::temp_dir().as_path()).unwrap();
        let out = tool
            .run("{\"query\":\"nonexistent\"}", &ctx)
            .await
            .expect("search runs");
        assert_eq!(out.as_text().unwrap(), "[]");
        assert!(discovered.lock().unwrap().is_empty());
    }
}

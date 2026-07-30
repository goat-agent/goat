mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::query::{KeySpec, LimitSpec, Residue, TermPolicy, WatchVocabulary};
use goat_integration::{IntegrationError, IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("notion");
pub const PREFIX: &str = "notion_";
pub const STREAM: &str = "view";

const MCP_URL: &str = "https://mcp.notion.com/mcp";

const SETUP: &str = "connects to Notion's hosted MCP server; a browser window will ask you to approve access.\n\
     to get briefed when work lands, declare a workflow in the agent's `watch` section, e.g.\n\
     { \"source\": \"notion\", \"query\": \"view:<url>\" } — the value is a saved Notion view URL (the one with ?v=).\n\
     known keys: view, limit; free text is not accepted.\n\
     without a watch entry the tools work and the watcher stays off.";

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "notion",
    residue: Residue::Reject,
    terms: TermPolicy::Reject,
    limit: Some(LimitSpec {
        default: watch::FETCH_LIMIT,
        max: 100,
    }),
    keys: &[KeySpec::new("view")],
};

pub fn service() -> McpService {
    McpService::new("notion", "Notion", ServiceUrl::Fixed(MCP_URL), SETUP)
        .tools(ToolPolicy::all(PREFIX))
        .truncation_hint("narrow the view, or request a smaller page")
        .compile(watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NotionBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub query_tool: Option<String>,
}

impl NotionBinding {
    pub(crate) fn read(config: &Value) -> Self {
        goat_integration_mcp::read_binding(config)
    }
}

fn meaningful<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    Ok(raw
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty()))
}

fn validate_config(config: &Value) -> IntegrationResult<()> {
    if let Some(object) = config.as_object()
        && object.contains_key("view_url")
    {
        return Err(IntegrationError::Config(
            "notion binding: `view_url` moved to the agent-level `watch` section; \
             write { \"source\": \"notion\", \"query\": \"view:<url>\" } there instead"
                .to_owned(),
        ));
    }
    goat_integration_mcp::validate_binding::<NotionBinding>("notion", config)
}

inventory::submit! {
    IntegrationFactory {
        id: ID,
        ctor: || Arc::new(service().build()),
        validate_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_integration::query::assert_vocabulary;
    use goat_integration::{Integration, IntegrationAuth, IntegrationBinding};
    use serde_json::json;

    #[test]
    fn the_vocabulary_holds_its_invariants() {
        assert_vocabulary(&VOCABULARY);
    }

    #[test]
    fn the_binding_is_typo_checked() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(validate_config(&json!({ "query_tool": "t" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "query_tool": 3 })).is_err());
        assert!(validate_config(&json!({ "viewurl": "x" })).is_err());
    }

    #[test]
    fn the_old_view_url_key_points_at_the_watch_section() {
        let err = validate_config(&json!({ "view_url": "https://notion.so/x?v=1" })).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("agent-level `watch` section"));
        assert!(message.contains("\"query\": \"view:<url>\""));
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("notion"));
        assert!(goat_integration::factory_for("notion").is_some());
    }

    #[test]
    fn metadata_has_no_environment_override_because_notion_offers_none() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "notion");
        assert_eq!(meta.display, "Notion");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, None);
        assert!(service().compile.is_some());
        assert!(service().defaults.is_none());
        assert!(meta.setup.contains("view:<url>"));
    }

    #[test]
    fn there_is_no_self_sufficient_default_watch() {
        let binding = IntegrationBinding::from_config(json!({}));
        assert!(service().build().default_watch(&binding).is_empty());
    }

    #[test]
    fn the_client_id_is_read_the_same_way_everywhere() {
        let binding = IntegrationBinding::from_config(json!({ "client_id": " cid " }));
        assert_eq!(
            goat_integration_mcp::client_id_of(&binding).as_deref(),
            Some("cid")
        );
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::REBUILD;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: STREAM.to_owned(),
            kind: IntegrationUpdateKind::Assigned,
            entity: "page",
            diff: REBUILD,
        })
        .await;
    }
}

mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{McpService, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("notion");
pub const PREFIX: &str = "notion_";

const MCP_URL: &str = "https://mcp.notion.com/mcp";

const SETUP: &str = "connects to Notion's hosted MCP server; a browser window will ask you to approve access.\nto get briefed when work lands, add `view_url` (a saved Notion view URL, the one with ?v=) to the agent's notion binding — without it the tools work and the watcher stays off";

pub fn service() -> McpService {
    McpService::new("notion", "Notion", ServiceUrl::Fixed(MCP_URL), SETUP)
        .tools(ToolPolicy::all(PREFIX))
        .truncation_hint("narrow the view, or request a smaller page")
        .watch(watch::spawn)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NotionBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub view_url: Option<String>,
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
    use goat_integration::{Integration, IntegrationAuth};
    use serde_json::json;

    #[test]
    fn the_binding_is_typo_checked() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(
            validate_config(&json!({ "view_url": "https://notion.so/x?v=1", "query_tool": "t" }))
                .is_ok()
        );
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "view_url": 3 })).is_err());
        assert!(validate_config(&json!({ "viewurl": "x" })).is_err());
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
        assert!(service().watch.is_some());
        assert!(meta.setup.contains("view_url"));
    }

    #[test]
    fn the_watcher_stays_off_until_a_view_is_named() {
        assert_eq!(NotionBinding::read(&json!({})).view_url, None);
        assert_eq!(
            NotionBinding::read(&json!({ "view_url": "  " })).view_url,
            None
        );
        assert_eq!(
            NotionBinding::read(&json!({ "view_url": " https://notion.so/x?v=1 " })).view_url,
            Some("https://notion.so/x?v=1".to_owned())
        );
    }

    #[test]
    fn the_client_id_is_read_the_same_way_everywhere() {
        let binding =
            goat_integration::IntegrationBinding::from_config(json!({ "client_id": " cid " }));
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
            stream: watch::STREAM.to_owned(),
            kind: IntegrationUpdateKind::Assigned,
            entity: "page",
            overflow_tail: "in the view",
            diff: REBUILD,
        })
        .await;
    }
}

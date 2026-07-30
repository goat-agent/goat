mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::McpService;
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("linear");
pub const PREFIX: &str = "linear_";

const MCP_URL: &str = "https://mcp.linear.app/mcp";
const ENV_VAR: &str = "LINEAR_API_KEY";

const SETUP: &str = "connects to Linear's hosted MCP server; a browser window will ask you to approve access.\n\
     the watcher briefs you on issues assigned to you — set `assignee`, `team`, or `project` in the agent's linear binding to watch something else.\n\
     to run headless, set LINEAR_API_KEY to a Linear personal API key.";

pub fn service() -> McpService {
    McpService::new("linear", "Linear", MCP_URL, SETUP)
        .with_env_var(ENV_VAR)
        .with_tool_prefix(PREFIX)
        .with_truncation_hint(
            "narrow the filter, request fewer issues, or fetch a single issue instead",
        )
        .with_watch(watch::spawn)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinearBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub assignee: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub team: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub project: Option<String>,
    #[serde(default)]
    pub include_closed: Option<bool>,
}

impl LinearBinding {
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
    goat_integration_mcp::validate_binding::<LinearBinding>("linear", config)
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
    fn the_watch_policy_is_configurable_and_typo_checked() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "client_id": "cid" })).is_ok());
        assert!(
            validate_config(&json!({ "assignee": "jmo", "team": "ENG", "include_closed": true }))
                .is_ok()
        );
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "assignee": 3 })).is_err());
        assert!(validate_config(&json!({ "asignee": "typo" })).is_err());
    }

    #[test]
    fn settings_are_trimmed_and_blank_values_ignored() {
        assert_eq!(LinearBinding::read(&json!({ "team": "  " })).team, None);
        assert_eq!(
            LinearBinding::read(&json!({ "team": " ENG " })).team,
            Some("ENG".to_owned())
        );
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("linear"));
        assert!(goat_integration::factory_for("linear").is_some());
    }

    #[test]
    fn metadata_advertises_oauth_with_a_headless_escape_hatch() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "linear");
        assert_eq!(meta.display, "Linear");
        assert_eq!(meta.auth, IntegrationAuth::OAuth);
        assert_eq!(meta.env_var, Some("LINEAR_API_KEY"));
        assert!(service().watch.is_some());
        assert!(meta.setup.contains("LINEAR_API_KEY"));
        assert!(meta.setup.contains("assignee"));
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
            entity: "issue",
            overflow_tail: "newly assigned",
            diff: REBUILD,
        })
        .await;
    }
}

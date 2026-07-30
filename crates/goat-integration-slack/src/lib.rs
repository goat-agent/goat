mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::{IntegrationFactory, IntegrationResult};
use goat_integration_mcp::McpService;
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("slack");
pub const PREFIX: &str = "slack_";

const MCP_URL: &str = "https://mcp.slack.com/mcp";
const ENV_VAR: &str = "SLACK_USER_TOKEN";

const SETUP: &str = "1. create the goat app in your workspace: https://api.slack.com/apps?new_app=1&manifest_yaml=display_information%3A%0A%20%20name%3A%20goat%0A%20%20description%3A%20Personal%20AI%20agent%20access%20to%20Slack%0Aoauth_config%3A%0A%20%20scopes%3A%0A%20%20%20%20user%3A%0A%20%20%20%20%20%20-%20search%3Aread.public%0A%20%20%20%20%20%20-%20search%3Aread.private%0A%20%20%20%20%20%20-%20search%3Aread.im%0A%20%20%20%20%20%20-%20search%3Aread.mpim%0A%20%20%20%20%20%20-%20search%3Aread.users%0A%20%20%20%20%20%20-%20channels%3Ahistory%0A%20%20%20%20%20%20-%20groups%3Ahistory%0A%20%20%20%20%20%20-%20im%3Ahistory%0A%20%20%20%20%20%20-%20mpim%3Ahistory%0A%20%20%20%20%20%20-%20users%3Aread%0A%20%20%20%20%20%20-%20emoji%3Aread%0A%20%20%20%20%20%20-%20chat%3Awrite%0A%20%20%20%20%20%20-%20reactions%3Awrite%0A%20%20%20%20%20%20-%20canvases%3Aread%0A%20%20%20%20%20%20-%20canvases%3Awrite%0Asettings%3A%0A%20%20org_deploy_enabled%3A%20false%0A%20%20socket_mode_enabled%3A%20false%0A%20%20token_rotation_enabled%3A%20false%0A\n2. in that app, open Agents & AI Apps and turn on the Slack MCP Server — scopes alone are not enough\n3. Install to Workspace, then copy the User OAuth Token (xoxp-…) from OAuth & Permissions";

pub fn service() -> McpService {
    McpService::new("slack", "Slack", MCP_URL, SETUP)
        .with_secret("Slack user OAuth token (xoxp-…)")
        .with_env_var(ENV_VAR)
        .with_tool_prefix(PREFIX)
        .with_truncation_hint("narrow the query, or search a single channel instead")
        .with_watch(watch::spawn)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SlackBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub user_id: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub query: Option<String>,
    #[serde(default, deserialize_with = "meaningful")]
    pub search_tool: Option<String>,
}

impl SlackBinding {
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
    goat_integration_mcp::validate_binding::<SlackBinding>("slack", config)
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
    fn a_typo_in_the_binding_is_rejected_rather_than_ignored() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "user_id": "U1" })).is_ok());
        assert!(validate_config(&json!({ "query": "in:#eng", "search_tool": "x" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "user_id": 3 })).is_err());
        assert!(validate_config(&json!({ "userid": "U1" })).is_err());
    }

    #[test]
    fn factory_is_registered_in_inventory() {
        let registry = goat_integration::registry_from_inventory();
        assert!(registry.contains_key("slack"));
        assert!(goat_integration::factory_for("slack").is_some());
    }

    #[test]
    fn metadata_uses_a_pasted_secret_not_an_oauth_round_trip() {
        let meta = service().build().metadata();
        assert_eq!(meta.id, "slack");
        assert_eq!(meta.display, "Slack");
        assert_eq!(meta.auth, IntegrationAuth::Secret);
        assert_eq!(meta.env_var, Some("SLACK_USER_TOKEN"));
        assert!(service().watch.is_some());
        assert!(meta.setup.contains("Agents & AI Apps"));
        assert!(meta.setup.contains("api.slack.com/apps?new_app=1"));
    }

    #[test]
    fn watcher_stays_off_until_the_owner_supplies_a_member_id() {
        assert_eq!(SlackBinding::read(&json!({ "user_id": "" })).user_id, None);
        assert_eq!(
            SlackBinding::read(&json!({ "user_id": "   " })).user_id,
            None
        );
        assert_eq!(
            SlackBinding::read(&json!({ "user_id": " U1 " })).user_id,
            Some("U1".to_owned())
        );
    }

    #[tokio::test]
    async fn the_watcher_honours_the_shared_contract() {
        use goat_integration::diff::RETAIN;
        use goat_integration::test_support::{WatchContract, assert_watch_contract};
        use goat_types::IntegrationUpdateKind;

        assert_watch_contract(&WatchContract {
            integration: ID,
            stream: watch::STREAM.to_owned(),
            kind: IntegrationUpdateKind::Updated,
            entity: "message",
            overflow_tail: "waiting on you",
            diff: RETAIN,
        })
        .await;
    }
}

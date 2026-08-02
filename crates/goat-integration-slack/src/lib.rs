mod parse;
mod watch;

use std::sync::Arc;

use goat_integration::query::{LimitSpec, Residue, TermPolicy, WatchVocabulary};
use goat_integration::{IntegrationError, IntegrationFactory, IntegrationResult};
use goat_integration_mcp::{AuthScheme, McpService, ServiceUrl, ToolPolicy};
use goat_types::IntegrationId;
use serde::Deserialize;
use serde_json::Value;

pub const ID: IntegrationId = IntegrationId::from_static("slack");
pub const PREFIX: &str = "slack_";

const MCP_URL: &str = "https://mcp.slack.com/mcp";
const ENV_VAR: &str = "SLACK_USER_TOKEN";

const SETUP: &str = "this integration connects as you; it does not use the Slack channel bot token (xoxb-…).\n\
1. create a separate goat integration app in your workspace: https://api.slack.com/apps?new_app=1&manifest_yaml=display_information%3A%0A%20%20name%3A%20goat%20integration%0A%20%20description%3A%20Personal%20AI%20agent%20access%20to%20Slack%0Aoauth_config%3A%0A%20%20scopes%3A%0A%20%20%20%20user%3A%0A%20%20%20%20%20%20-%20search%3Aread.public%0A%20%20%20%20%20%20-%20search%3Aread.private%0A%20%20%20%20%20%20-%20search%3Aread.im%0A%20%20%20%20%20%20-%20search%3Aread.mpim%0A%20%20%20%20%20%20-%20search%3Aread.users%0A%20%20%20%20%20%20-%20channels%3Ahistory%0A%20%20%20%20%20%20-%20groups%3Ahistory%0A%20%20%20%20%20%20-%20im%3Ahistory%0A%20%20%20%20%20%20-%20mpim%3Ahistory%0A%20%20%20%20%20%20-%20users%3Aread%0A%20%20%20%20%20%20-%20emoji%3Aread%0A%20%20%20%20%20%20-%20chat%3Awrite%0A%20%20%20%20%20%20-%20reactions%3Awrite%0A%20%20%20%20%20%20-%20canvases%3Aread%0A%20%20%20%20%20%20-%20canvases%3Awrite%0Asettings%3A%0A%20%20org_deploy_enabled%3A%20false%0A%20%20socket_mode_enabled%3A%20false%0A%20%20token_rotation_enabled%3A%20false%0A\n2. under OAuth & Permissions, confirm the permissions appear under User Token Scopes, not Bot Token Scopes\n3. open Agents & AI Apps and turn on the Slack MCP Server — scopes alone are not enough\n4. click Install to Workspace, or Reinstall to Workspace if the app was already installed\n5. under OAuth & Permissions → OAuth Tokens for Your Workspace, copy the User OAuth Token (xoxp-…); do not use the Bot User OAuth Token (xoxb-…)\n\
6. set `user_id` to your Slack member ID in the agent's slack binding — the watcher stays off until it is.\n\
by default the watcher briefs you on messages that mention you (`@me`).\n\
declare workflows in the agent's `watch` section to change that, e.g.\n\
{ \"source\": \"slack\", \"query\": \"@me in:#eng limit:25\" } —\n\
Slack's native search modifiers (from:, in:, has:, before:, \"quoted phrases\") pass through verbatim,\n\
`@me` becomes your member mention, and limit caps the fetch (default 50, max 100).";

pub const VOCABULARY: WatchVocabulary = WatchVocabulary {
    integration: "slack",
    residue: Residue::Keep,
    terms: TermPolicy::Reject,
    limit: Some(LimitSpec {
        default: 50,
        max: 100,
    }),
    keys: &[],
};

pub fn service() -> McpService {
    McpService::new("slack", "Slack", ServiceUrl::Fixed(MCP_URL), SETUP)
        .secret(
            "Slack User OAuth Token (xoxp-…; not the xoxb-… bot token)",
            AuthScheme::Raw,
        )
        .env_var(ENV_VAR)
        .tools(ToolPolicy::all(PREFIX))
        .truncation_hint("narrow the query, or search a single channel instead")
        .defaults(watch::defaults)
        .watch(&VOCABULARY, watch::compile)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SlackBinding {
    #[serde(default, deserialize_with = "meaningful")]
    pub user_id: Option<String>,
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

const MOVED_KEYS: &[&str] = &["query"];

fn validate_config(config: &Value) -> IntegrationResult<()> {
    if let Some(object) = config.as_object() {
        for key in MOVED_KEYS {
            if object.contains_key(*key) {
                return Err(IntegrationError::Config(format!(
                    "slack binding: `{key}` moved to the agent-level `watch` section; \
                     write {{ \"source\": \"slack\", \"query\": \"...\" }} there instead"
                )));
            }
        }
    }
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
    use goat_integration::query::assert_vocabulary;
    use goat_integration::{Integration, IntegrationAuth};
    use serde_json::json;

    #[test]
    fn the_vocabulary_holds_its_invariants() {
        assert_vocabulary(&VOCABULARY);
    }

    #[test]
    fn a_typo_in_the_binding_is_rejected_rather_than_ignored() {
        assert!(validate_config(&json!({})).is_ok());
        assert!(validate_config(&json!({ "account": "work", "user_id": "U1" })).is_ok());
        assert!(validate_config(&json!({ "search_tool": "x" })).is_ok());
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "user_id": 3 })).is_err());
        assert!(validate_config(&json!({ "userid": "U1" })).is_err());
    }

    #[test]
    fn the_old_query_key_points_at_the_watch_section() {
        let err = validate_config(&json!({ "query": "in:#eng" })).unwrap_err();
        assert!(err.to_string().contains("agent-level `watch` section"));
        assert!(err.to_string().contains("\"source\": \"slack\""));
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
        assert!(service().compile.is_some());
        assert!(service().defaults.is_some());
        assert!(meta.setup.contains("Agents & AI Apps"));
        assert!(meta.setup.contains("api.slack.com/apps?new_app=1"));
        assert!(meta.setup.contains("User Token Scopes"));
        assert!(meta.setup.contains("Reinstall to Workspace"));
        assert!(meta.setup.contains("do not use the Bot User OAuth Token"));
        assert!(meta.setup.contains("`@me`"));
        assert!(meta.setup.contains("`watch` section"));
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
            diff: RETAIN,
        })
        .await;
    }
}

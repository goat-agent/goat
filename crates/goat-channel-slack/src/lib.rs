mod api;
mod channel;
mod config;
mod conversation;
mod handle;
mod inbound;
mod mrkdwn;
mod socket;

use std::sync::Arc;
use std::time::Duration;

use goat_channel::{
    ChannelCapabilities, ChannelError, ChannelFactory, ChannelMetadata, ChannelResult, SecretSpec,
};
use goat_types::ChannelId;

pub use channel::SlackChannel;

pub const ID: ChannelId = ChannelId::from_static("slack");

pub(crate) const BOT_TOKEN_SLOT: &str = "bot_token";
pub(crate) const APP_TOKEN_SLOT: &str = "app_token";

pub(crate) const CAPABILITIES: ChannelCapabilities =
    ChannelCapabilities::new(3900, Duration::from_secs(1), None);

const SETUP: &str = "\
Slack needs two tokens: a bot token to speak, and an app-level token to open the
socket. Create one app that carries both.

1. open https://api.slack.com/apps, choose Create New App > From a manifest,
   pick your workspace, and paste:

     display_information:
       name: goat
       description: Personal AI agent
     features:
       bot_user:
         display_name: goat
         always_online: true
       app_home:
         home_tab_enabled: false
         messages_tab_enabled: true
         messages_tab_read_only_enabled: false
     oauth_config:
       scopes:
         bot:
           - chat:write
           - channels:history
           - groups:history
           - im:history
           - mpim:history
           - users:read
     settings:
       event_subscriptions:
         bot_events:
           - message.channels
           - message.groups
           - message.im
           - message.mpim
       socket_mode_enabled: true
       org_deploy_enabled: false
       token_rotation_enabled: false

2. Basic Information > App-Level Tokens > Generate: add the `connections:write`
   scope and copy the `xapp-` token
3. Install App > Install to Workspace, then copy the Bot User OAuth Token
   (`xoxb-`) from OAuth & Permissions
4. invite the bot to any channel you want it to read: /invite @goat";

const SECRETS: &[SecretSpec] = &[
    SecretSpec::new(BOT_TOKEN_SLOT, "Slack bot token (xoxb-…)"),
    SecretSpec::new(APP_TOKEN_SLOT, "Slack app-level token (xapp-…)"),
];

fn metadata() -> ChannelMetadata {
    ChannelMetadata::new("Slack", SETUP, SECRETS)
}

inventory::submit! {
    ChannelFactory { id: ID, ctor: || Arc::new(SlackChannel), validate_config, metadata }
}

fn validate_config(value: &serde_json::Value) -> ChannelResult<()> {
    serde_json::from_value::<config::SlackConfig>(value.clone())
        .map(|_| ())
        .map_err(|e| ChannelError::Config(format!("slack: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_factory_is_registered_under_slack() {
        let factory = goat_channel::factory_for("slack").expect("slack factory");
        assert_eq!(factory.id.as_str(), "slack");
    }

    #[test]
    fn metadata_declares_both_tokens_in_prompt_order() {
        let metadata = metadata();
        assert_eq!(metadata.display, "Slack");
        assert_eq!(metadata.secrets.len(), 2);
        assert_eq!(metadata.secrets[0].slot, BOT_TOKEN_SLOT);
        assert_eq!(metadata.secrets[1].slot, APP_TOKEN_SLOT);
        assert!(metadata.secrets[0].label.contains("xoxb-"));
        assert!(metadata.secrets[1].label.contains("xapp-"));
    }

    #[test]
    fn the_setup_manifest_turns_socket_mode_on_and_asks_for_connections_write() {
        let setup = metadata().setup;
        assert!(setup.contains("socket_mode_enabled: true"));
        assert!(setup.contains("connections:write"));
        assert!(setup.contains("messages_tab_enabled: true"));
    }

    #[test]
    fn the_setup_manifest_subscribes_to_every_message_surface() {
        let setup = metadata().setup;
        for event in [
            "message.channels",
            "message.groups",
            "message.im",
            "message.mpim",
        ] {
            assert!(setup.contains(event), "{event} should be subscribed");
        }
    }

    #[test]
    fn the_setup_manifest_does_not_ask_for_app_mention() {
        let setup = metadata().setup;
        assert!(!setup.contains("app_mention"));
        assert!(!setup.contains("app_mentions:read"));
    }

    #[test]
    fn capabilities_match_slack_limits() {
        assert_eq!(CAPABILITIES.max_message_chars, 3900);
        assert_eq!(CAPABILITIES.edit_min_interval, Duration::from_secs(1));
        assert!(CAPABILITIES.typing_refresh.is_none());
    }

    #[test]
    fn config_preserves_missing_and_empty_allowlists() {
        let missing: config::SlackConfig = serde_json::from_value(json!({})).unwrap();
        let empty: config::SlackConfig =
            serde_json::from_value(json!({ "allowed_user_ids": [] })).unwrap();
        assert!(missing.allowed_user_ids.is_none());
        assert!(empty.allowed_user_ids.is_some_and(|ids| ids.is_empty()));
        assert!(validate_config(&json!({ "allowed_user_ids": ["U1", "U2"] })).is_ok());
    }

    #[test]
    fn config_never_holds_a_token() {
        assert!(validate_config(&json!({ "bot_token": "xoxb-1" })).is_err());
        assert!(validate_config(&json!({ "app_token": "xapp-1" })).is_err());
    }

    #[test]
    fn config_rejects_the_wrong_shapes() {
        assert!(validate_config(&json!("nope")).is_err());
        assert!(validate_config(&json!({ "allowed_user_ids": "U1" })).is_err());
        assert!(validate_config(&json!({ "allowed_user_ids": [1] })).is_err());
    }
}

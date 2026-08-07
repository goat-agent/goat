mod channel;
mod config;
mod handle;
mod inbound;
mod interaction;

use std::sync::Arc;
use std::time::Duration;

use goat_channel::{
    ChannelCapabilities, ChannelError, ChannelFactory, ChannelMetadata, ChannelResult, SecretSpec,
};
use goat_types::ChannelId;

pub use channel::DiscordChannel;

pub const ID: ChannelId = ChannelId::from_static("discord");
pub(crate) const TOKEN_SLOT: &str = "token";

pub(crate) const CAPABILITIES: ChannelCapabilities = ChannelCapabilities::new(
    2000,
    Duration::from_millis(250),
    Some(Duration::from_secs(8)),
);

const SETUP: &str = "\
Create the bot, then paste its token.

1. open https://discord.com/developers/applications and create an application
2. under Bot, enable MESSAGE CONTENT INTENT (Privileged Gateway Intents)
3. under Bot, Reset Token and copy it
4. under OAuth2 > URL Generator pick the `bot` scope, then open the generated
   URL to invite the bot to your server";

const SECRETS: &[SecretSpec] = &[SecretSpec::new(TOKEN_SLOT, "Discord bot token")];

fn metadata() -> ChannelMetadata {
    ChannelMetadata::new("Discord", SETUP, SECRETS)
}

inventory::submit! {
    ChannelFactory { id: ID, ctor: || Arc::new(DiscordChannel), validate_config, metadata }
}

fn validate_config(value: &serde_json::Value) -> ChannelResult<()> {
    serde_json::from_value::<config::DiscordConfig>(value.clone())
        .map(|_| ())
        .map_err(|e| ChannelError::Config(format!("discord: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factory_is_registered_with_metadata() {
        let factory = goat_channel::factory_for("discord").expect("discord factory");
        let metadata = (factory.metadata)();
        assert_eq!(metadata.display, "Discord");
        assert_eq!(metadata.secrets.len(), 1);
        assert_eq!(metadata.secrets[0].slot, TOKEN_SLOT);
        assert!(metadata.setup.contains("MESSAGE CONTENT INTENT"));
    }

    #[test]
    fn config_preserves_missing_and_empty_allowlists() {
        let missing: config::DiscordConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        let empty: config::DiscordConfig =
            serde_json::from_value(serde_json::json!({ "allowed_user_ids": [] })).unwrap();
        assert!(missing.allowed_user_ids.is_none());
        assert!(empty.allowed_user_ids.is_some_and(|ids| ids.is_empty()));
        assert!(validate_config(&serde_json::json!({ "allowed_user_ids": [1] })).is_ok());
    }

    #[test]
    fn config_no_longer_accepts_a_token() {
        assert!(validate_config(&serde_json::json!({ "token": "x" })).is_err());
    }

    #[test]
    fn config_rejects_the_wrong_shapes() {
        assert!(validate_config(&serde_json::json!("nope")).is_err());
        assert!(validate_config(&serde_json::json!({ "intents": "GUILDS" })).is_err());
        assert!(validate_config(&serde_json::json!({ "allowed_user_ids": ["1"] })).is_err());
    }
}

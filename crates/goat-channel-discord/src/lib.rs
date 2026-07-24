mod channel;
mod config;
mod handle;
mod inbound;
mod interaction;

use std::sync::Arc;
use std::time::Duration;

use goat_channel::{ChannelCapabilities, ChannelError, ChannelFactory, ChannelResult};
use goat_types::ChannelId;

pub use channel::DiscordChannel;

pub const ID: ChannelId = ChannelId::from_static("discord");
pub(crate) const CAPABILITIES: ChannelCapabilities = ChannelCapabilities::new(
    2000,
    Duration::from_millis(250),
    Some(Duration::from_secs(8)),
);

inventory::submit! {
    ChannelFactory { id: ID, ctor: || Arc::new(DiscordChannel), validate_config }
}

fn validate_config(value: &serde_json::Value) -> ChannelResult<()> {
    serde_json::from_value::<config::DiscordConfig>(value.clone())
        .map(|_| ())
        .map_err(|e| ChannelError::Config(format!("discord: {e}")))
}

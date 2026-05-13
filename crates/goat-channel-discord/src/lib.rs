mod channel;
mod config;
mod handle;
mod inbound;
mod interaction;

use std::sync::Arc;
use std::time::Duration;

use goat_channel::{ChannelCapabilities, ChannelFactory};
use goat_types::ChannelId;

pub use channel::DiscordChannel;

pub const ID: ChannelId = ChannelId::from_static("discord");
pub(crate) const CAPABILITIES: ChannelCapabilities = ChannelCapabilities::new(
    2000,
    Duration::from_millis(250),
    Some(Duration::from_secs(8)),
);

inventory::submit! {
    ChannelFactory { id: ID, ctor: || Arc::new(DiscordChannel) }
}

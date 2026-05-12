mod channel;
mod config;
mod handle;
mod inbound;

use std::sync::Arc;
use std::time::Duration;

use goat_channel::{ChannelCapabilities, ChannelFactory};
use goat_types::ChannelId;

pub use channel::TelegramChannel;

pub const ID: ChannelId = ChannelId::from_static("telegram");
pub(crate) const CAPABILITIES: ChannelCapabilities = ChannelCapabilities::new(
    4096,
    Duration::from_millis(1500),
    Some(Duration::from_secs(4)),
);

inventory::submit! {
    ChannelFactory { id: ID, ctor: || Arc::new(TelegramChannel) }
}

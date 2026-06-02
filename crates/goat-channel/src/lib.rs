use std::sync::Arc;

use async_trait::async_trait;
use goat_command::CommandSpec;
use goat_types::{
    ChannelId, ConversationId, IncomingMessage, InstanceId, MessageId, OutgoingBody, PersonaId,
};
use thiserror::Error;
use tokio::sync::mpsc;

mod typing;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use typing::{spawn_typing, TypingGuard};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChannelError {
    #[error("auth: {0}")]
    Auth(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("config: {0}")]
    Config(String),
    #[error("provider error: {0}")]
    Provider(String),
}

pub type ChannelResult<T> = Result<T, ChannelError>;

#[derive(Clone, Debug)]
pub struct ChannelBinding {
    pub instance: InstanceId,
    pub config: serde_json::Value,
    pub commands: Vec<CommandSpec>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ChannelIdentity {
    pub handle: String,
    pub display: String,
    pub avatar: Option<url::Url>,
}

impl ChannelIdentity {
    pub fn new(handle: impl Into<String>, display: impl Into<String>) -> Self {
        Self {
            handle: handle.into(),
            display: display.into(),
            avatar: None,
        }
    }

    pub fn with_avatar(mut self, avatar: url::Url) -> Self {
        self.avatar = Some(avatar);
        self
    }
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ChannelCapabilities {
    pub max_message_chars: usize,
    pub edit_min_interval: std::time::Duration,
    pub typing_refresh: Option<std::time::Duration>,
}

impl ChannelCapabilities {
    pub const fn new(
        max_message_chars: usize,
        edit_min_interval: std::time::Duration,
        typing_refresh: Option<std::time::Duration>,
    ) -> Self {
        Self {
            max_message_chars,
            edit_min_interval,
            typing_refresh,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SentRef {
    pub channel: ChannelId,
    pub message_id: MessageId,
    pub raw: serde_json::Value,
}

pub struct ChannelTurn {
    pub reply_to: Option<MessageId>,
    pub typing: TypingGuard,
}

pub type BindOutput = (Arc<dyn ChannelHandle>, mpsc::Receiver<IncomingMessage>);

/// Represents a chat channel that a persona can be bound to.
///
/// Implement this trait in a dedicated `goat-channel-<channel>` crate.
/// Channel-specific networking, auth, and message formatting stay inside
/// that crate. A *channel* is a platform where goat has its own addressable
/// identity (e.g. a Discord bot, a Telegram bot). If the integration does
/// not give goat an addressable identity, it is a *plugin*, not a channel.
///
/// Register your implementation via the inventory system so the runtime
/// discovers it at link time:
/// ```ignore
/// inventory::submit!(ChannelFactory { id: MY_CHANNEL_ID, ctor: ..., validate_config: ... });
/// ```
///
/// `bind` is called once per persona that has this channel configured. It
/// returns a [`ChannelHandle`] for outbound operations and an mpsc receiver
/// for inbound messages.
#[async_trait]
pub trait Channel: Send + Sync + 'static {
    /// Returns the channel's unique identifier. Must match the `ChannelId`
    /// constant in your `ChannelFactory` registration.
    fn id(&self) -> ChannelId;

    /// Binds the channel to a specific persona and configuration, starting
    /// any background polling/websocket tasks required to receive messages.
    /// Returns `(handle, receiver)` — the handle owns outbound operations and
    /// the receiver delivers inbound messages to the brain.
    async fn bind(
        self: Arc<Self>,
        persona: PersonaId,
        binding: ChannelBinding,
    ) -> ChannelResult<BindOutput>;
}

/// Owned handle to a bound channel instance, shared by the brain for all
/// outbound operations on a single persona+channel binding.
///
/// Channel crates implement this trait. The brain holds an `Arc<dyn ChannelHandle>`
/// and never interacts with the underlying channel implementation directly.
///
/// All methods that can fail return [`ChannelResult`]. Implementations must not
/// block indefinitely — use appropriate network timeouts internally.
///
/// The [`prepare_turn`] method has a default implementation that starts a typing
/// indicator and sets the reply target; override it only if your channel requires
/// different turn-start behaviour.
#[async_trait]
pub trait ChannelHandle: Send + Sync + 'static {
    /// The unique binding instance identifier (stable across daemon restarts
    /// for the same persona+channel+config triple).
    fn instance(&self) -> InstanceId;
    fn persona(&self) -> PersonaId;
    fn id(&self) -> ChannelId;
    fn identity(&self) -> ChannelIdentity;
    fn capabilities(&self) -> ChannelCapabilities;

    /// Sends a message to the given conversation, optionally replying to
    /// `reply_to`. Returns a [`SentRef`] that can be used to edit the message.
    async fn send(
        &self,
        conv: &ConversationId,
        body: OutgoingBody,
        reply_to: Option<MessageId>,
    ) -> ChannelResult<SentRef>;

    /// Edits a previously sent message. No-op channels may return
    /// `ChannelError::Unsupported`.
    async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()>;

    /// Starts or refreshes a typing indicator for the given conversation.
    async fn typing(&self, conv: &ConversationId) -> ChannelResult<TypingGuard>;

    /// Called at the beginning of every incoming turn. The default
    /// implementation starts a typing indicator and sets `reply_to` to the
    /// incoming message id. Override only when your channel requires different
    /// turn-start semantics.
    async fn prepare_turn(&self, msg: &IncomingMessage) -> ChannelResult<ChannelTurn> {
        Ok(ChannelTurn {
            reply_to: Some(msg.id.clone()),
            typing: self.typing(&msg.conversation).await?,
        })
    }
}

pub struct ChannelFactory {
    pub id: ChannelId,
    pub ctor: fn() -> Arc<dyn Channel>,
    pub validate_config: fn(&serde_json::Value) -> ChannelResult<()>,
}

inventory::collect!(ChannelFactory);

use std::sync::Arc;

use async_trait::async_trait;
use goat_agent_command::CommandSpec;
use goat_types::{
    AgentId, ChannelId, IncomingMessage, InstanceId, MessageId, OutgoingBody, ThreadId,
};
use thiserror::Error;
use tokio::sync::mpsc;

mod secrets;
mod typing;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use secrets::{
    ChannelMetadata, ChannelSecrets, SecretSpec, forget as forget_secrets, load as load_secrets,
    save as save_secrets, secret_key,
};
pub use typing::{TypingGuard, spawn_typing};

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
    pub secrets: ChannelSecrets,
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

    #[must_use]
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

#[async_trait]
pub trait Channel: Send + Sync + 'static {
    fn id(&self) -> ChannelId;

    async fn bind(
        self: Arc<Self>,
        agent: AgentId,
        binding: ChannelBinding,
    ) -> ChannelResult<BindOutput>;

    async fn verify(
        &self,
        config: &serde_json::Value,
        secrets: &ChannelSecrets,
    ) -> ChannelResult<ChannelIdentity>;
}

#[async_trait]
pub trait ChannelHandle: Send + Sync + 'static {
    fn instance(&self) -> InstanceId;
    fn agent(&self) -> AgentId;
    fn id(&self) -> ChannelId;
    fn identity(&self) -> ChannelIdentity;
    fn capabilities(&self) -> ChannelCapabilities;

    async fn send(
        &self,
        conv: &ThreadId,
        body: OutgoingBody,
        reply_to: Option<MessageId>,
    ) -> ChannelResult<SentRef>;

    async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()>;

    async fn typing(&self, conv: &ThreadId) -> ChannelResult<TypingGuard>;

    async fn prepare_turn(&self, msg: &IncomingMessage) -> ChannelResult<ChannelTurn> {
        Ok(ChannelTurn {
            reply_to: Some(msg.id.clone()),
            typing: self.typing(&msg.thread).await?,
        })
    }

    fn supports_threads(&self) -> bool {
        false
    }

    async fn open_thread(
        &self,
        parent: &ThreadId,
        anchor: Option<&MessageId>,
        title: &str,
    ) -> ChannelResult<ThreadId> {
        let _ = (parent, anchor, title);
        Err(ChannelError::Unsupported("open_thread not supported"))
    }
}

pub struct ChannelFactory {
    pub id: ChannelId,
    pub ctor: fn() -> Arc<dyn Channel>,
    pub validate_config: fn(&serde_json::Value) -> ChannelResult<()>,
    pub metadata: fn() -> ChannelMetadata,
}

inventory::collect!(ChannelFactory);

#[must_use]
pub fn factory_for(id: &str) -> Option<&'static ChannelFactory> {
    inventory::iter::<ChannelFactory>().find(|factory| factory.id.as_str() == id)
}

#[must_use]
pub fn registered_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = inventory::iter::<ChannelFactory>()
        .map(|factory| factory.id.as_str())
        .collect();
    ids.sort_unstable();
    ids
}

#[must_use]
pub fn metadata_for(id: &str) -> Option<ChannelMetadata> {
    factory_for(id).map(|factory| (factory.metadata)())
}

#[must_use]
pub fn secret_specs(id: &str) -> &'static [SecretSpec] {
    metadata_for(id).map_or(&[], |metadata| metadata.secrets)
}

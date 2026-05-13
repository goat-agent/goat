use std::borrow::Cow;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const GOAT_NAMESPACE: Uuid = Uuid::from_u128(0x6f61_745f_7065_7273_6f6e_615f_6e73_3031);

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub struct PersonaId(pub Uuid);

impl PersonaId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_slug(slug: &str) -> Self {
        Self(Uuid::new_v5(&GOAT_NAMESPACE, slug.as_bytes()))
    }
}

impl Default for PersonaId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PersonaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub struct InstanceId(pub Uuid);

impl InstanceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub struct ChannelId(Cow<'static, str>);

impl ChannelId {
    pub const fn from_static(slug: &'static str) -> Self {
        Self(Cow::Borrowed(slug))
    }

    pub fn new(slug: impl Into<String>) -> Self {
        Self(Cow::Owned(slug.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub struct ConversationId {
    pub channel: ChannelId,
    pub instance: InstanceId,
    pub external: String,
}

impl ConversationId {
    pub fn new(channel: ChannelId, instance: InstanceId, external: impl Into<String>) -> Self {
        Self {
            channel,
            instance,
            external: external.into(),
        }
    }

    pub fn to_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.channel.as_str(),
            self.instance.0,
            self.external
        )
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_key())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserHandle {
    pub external: String,
    pub display: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub mime: String,
    pub name: Option<String>,
    pub size: u64,
    pub source: AttachmentSource,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum AttachmentSource {
    Url(String),
    ChannelRef {
        channel: ChannelId,
        kind: String,
        value: String,
        raw: serde_json::Value,
    },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CommandName(Cow<'static, str>);

impl CommandName {
    pub const fn from_static(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    pub fn new(name: impl Into<String>) -> Result<Self, InvalidCommandName> {
        let name = name.into();
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(InvalidCommandName(name));
        }
        Ok(Self(Cow::Owned(name)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommandName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
#[error("invalid command name `{0}`")]
pub struct InvalidCommandName(pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommandCall {
    pub call_id: String,
    pub name: CommandName,
    pub args: String,
    pub raw: serde_json::Value,
}

impl CommandCall {
    pub fn new(
        call_id: impl Into<String>,
        name: CommandName,
        args: impl Into<String>,
        raw: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name,
            args: args.into(),
            raw,
        }
    }
}

#[derive(Clone, Debug)]
pub struct IncomingMessage {
    pub id: MessageId,
    pub persona: PersonaId,
    pub conversation: ConversationId,
    pub from: UserHandle,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub command: Option<CommandCall>,
    pub ts: DateTime<Utc>,
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum OutgoingBody {
    Text(String),
    File(Attachment),
    Reaction { target: MessageId, emoji: String },
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    Incoming(IncomingMessage),
}

impl Event {
    pub fn persona(&self) -> PersonaId {
        match self {
            Event::Incoming(m) => m.persona,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_key_round_trip() {
        let instance = InstanceId::new();
        let id = ConversationId::new(ChannelId::new("test"), instance, "chat:123:thread:5");
        let key = id.to_key();
        assert!(key.starts_with("test:"));
        assert!(key.ends_with(":chat:123:thread:5"));
        assert!(key.contains(&instance.0.to_string()));
    }

    #[test]
    fn event_persona_matches_message() {
        let p = PersonaId::new();
        let msg = IncomingMessage {
            id: MessageId("m1".into()),
            persona: p,
            conversation: ConversationId::new(ChannelId::new("test"), InstanceId::new(), "x"),
            from: UserHandle {
                external: "u".into(),
                display: None,
            },
            text: "hi".into(),
            attachments: vec![],
            command: None,
            ts: Utc::now(),
            raw: serde_json::Value::Null,
        };
        assert_eq!(Event::Incoming(msg).persona(), p);
    }
}

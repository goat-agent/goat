use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use goat_channel::{
    spawn_typing, ChannelCapabilities, ChannelError, ChannelHandle, ChannelIdentity, ChannelResult,
    SentRef, TypingGuard,
};
use goat_types::{ChannelId, ConversationId, InstanceId, MessageId, OutgoingBody, PersonaId};
use twilight_http::Client as HttpClient;
use twilight_model::id::marker::{ChannelMarker, MessageMarker};
use twilight_model::id::Id;

use crate::{CAPABILITIES, ID};

pub(crate) struct DiscordHandle {
    instance: InstanceId,
    persona: PersonaId,
    identity: ChannelIdentity,
    http: Arc<HttpClient>,
}

impl DiscordHandle {
    pub(crate) fn new(
        instance: InstanceId,
        persona: PersonaId,
        identity: ChannelIdentity,
        http: Arc<HttpClient>,
    ) -> Self {
        Self {
            instance,
            persona,
            identity,
            http,
        }
    }
}

#[async_trait]
impl ChannelHandle for DiscordHandle {
    fn instance(&self) -> InstanceId {
        self.instance
    }

    fn persona(&self) -> PersonaId {
        self.persona
    }

    fn id(&self) -> ChannelId {
        ID.clone()
    }

    fn identity(&self) -> ChannelIdentity {
        self.identity.clone()
    }

    fn capabilities(&self) -> ChannelCapabilities {
        CAPABILITIES
    }

    async fn send(
        &self,
        conv: &ConversationId,
        body: OutgoingBody,
        _reply_to: Option<MessageId>,
    ) -> ChannelResult<SentRef> {
        let channel_id = parse_channel_id(&conv.external)?;
        match body {
            OutgoingBody::Text(text) => {
                let resp = self
                    .http
                    .create_message(channel_id)
                    .content(&text)
                    .await
                    .map_err(|e| ChannelError::Provider(e.to_string()))?;
                let model = resp
                    .model()
                    .await
                    .map_err(|e| ChannelError::Provider(e.to_string()))?;
                Ok(sent_ref(channel_id, model.id))
            }
            OutgoingBody::File(_) | OutgoingBody::Reaction { .. } => {
                Err(ChannelError::Unsupported("discord: v0 only supports text"))
            }
            _ => Err(ChannelError::Unsupported("discord: unknown outgoing body")),
        }
    }

    async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()> {
        let (channel_id, message_id) = parse_sent_ref(sent)?;
        match body {
            OutgoingBody::Text(text) => {
                self.http
                    .update_message(channel_id, message_id)
                    .content(Some(&text))
                    .await
                    .map_err(|e| ChannelError::Provider(e.to_string()))?;
                Ok(())
            }
            _ => Err(ChannelError::Unsupported(
                "discord: edit only supports text",
            )),
        }
    }

    async fn typing(&self, conv: &ConversationId) -> ChannelResult<TypingGuard> {
        let channel_id = parse_channel_id(&conv.external)?;
        let http = self.http.clone();
        let refresh = CAPABILITIES
            .typing_refresh
            .unwrap_or(Duration::from_secs(8));
        Ok(spawn_typing(refresh, move || {
            let http = http.clone();
            async move {
                let _ = http.create_typing_trigger(channel_id).await;
            }
        }))
    }
}

fn parse_channel_id(s: &str) -> ChannelResult<Id<ChannelMarker>> {
    let raw = if let Some(rest) = s.strip_prefix("dm:") {
        rest
    } else if let Some(rest) = s.strip_prefix("chan:") {
        rest
    } else if let Some(rest) = s.rsplit(":c:").next() {
        rest
    } else {
        return Err(ChannelError::BadRequest(format!(
            "not a discord conversation: {s}"
        )));
    };
    raw.parse::<u64>()
        .map(Id::new)
        .map_err(|e| ChannelError::BadRequest(format!("bad channel_id: {e}")))
}

fn parse_sent_ref(sent: &SentRef) -> ChannelResult<(Id<ChannelMarker>, Id<MessageMarker>)> {
    let channel = sent.raw["channel_id"]
        .as_str()
        .ok_or_else(|| ChannelError::BadRequest("missing channel_id".into()))?
        .parse::<u64>()
        .map_err(|e| ChannelError::BadRequest(format!("bad channel_id: {e}")))?;
    let message = sent.raw["message_id"]
        .as_str()
        .ok_or_else(|| ChannelError::BadRequest("missing message_id".into()))?
        .parse::<u64>()
        .map_err(|e| ChannelError::BadRequest(format!("bad message_id: {e}")))?;
    Ok((Id::new(channel), Id::new(message)))
}

fn sent_ref(channel_id: Id<ChannelMarker>, message_id: Id<MessageMarker>) -> SentRef {
    SentRef {
        channel: ID.clone(),
        message_id: MessageId(message_id.to_string()),
        raw: serde_json::json!({
            "channel_id": channel_id.to_string(),
            "message_id": message_id.to_string(),
        }),
    }
}

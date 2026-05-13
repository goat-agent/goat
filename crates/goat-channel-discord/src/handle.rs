use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use goat_channel::{
    spawn_typing, ChannelCapabilities, ChannelError, ChannelHandle, ChannelIdentity, ChannelResult,
    ChannelTurn, SentRef, TypingGuard,
};
use goat_types::{
    ChannelId, ConversationId, IncomingMessage, InstanceId, MessageId, OutgoingBody, PersonaId,
};
use twilight_http::Client as HttpClient;
use twilight_model::id::marker::{ChannelMarker, MessageMarker};
use twilight_model::id::Id;

use crate::interaction::{InteractionResponseRef, InteractionState, PendingInteraction};
use crate::{CAPABILITIES, ID};

const INTERACTION_RESPONSE_KIND: &str = "discord_interaction_response";

pub(crate) struct DiscordHandle {
    instance: InstanceId,
    persona: PersonaId,
    identity: ChannelIdentity,
    http: Arc<HttpClient>,
    interactions: Arc<InteractionState>,
}

impl DiscordHandle {
    pub(crate) fn new(
        instance: InstanceId,
        persona: PersonaId,
        identity: ChannelIdentity,
        http: Arc<HttpClient>,
        interactions: Arc<InteractionState>,
    ) -> Self {
        Self {
            instance,
            persona,
            identity,
            http,
            interactions,
        }
    }

    async fn take_pending_interaction(
        &self,
        reply_to: Option<&MessageId>,
    ) -> Option<PendingInteraction> {
        let reply_to = reply_to?;
        self.interactions.take_pending(reply_to).await
    }

    async fn send_interaction_response(
        &self,
        pending: PendingInteraction,
        text: String,
    ) -> ChannelResult<SentRef> {
        let model = self
            .http
            .interaction(pending.application_id)
            .update_response(&pending.token)
            .content(Some(&text))
            .await
            .map_err(|e| ChannelError::Provider(e.to_string()))?
            .model()
            .await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;
        let sent = interaction_sent_ref(pending.channel_id, model.id);
        self.interactions
            .insert_response(
                sent.message_id.clone(),
                InteractionResponseRef {
                    application_id: pending.application_id,
                    token: pending.token,
                },
            )
            .await;
        Ok(sent)
    }

    async fn edit_interaction_response(&self, sent: &SentRef, text: String) -> ChannelResult<()> {
        let response = self
            .interactions
            .response(&sent.message_id)
            .await
            .ok_or_else(|| {
                ChannelError::BadRequest("unknown discord interaction response".into())
            })?;
        self.http
            .interaction(response.application_id)
            .update_response(&response.token)
            .content(Some(&text))
            .await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;
        Ok(())
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
        reply_to: Option<MessageId>,
    ) -> ChannelResult<SentRef> {
        let channel_id = parse_channel_id(&conv.external)?;
        match body {
            OutgoingBody::Text(text) => {
                if let Some(pending) = self.take_pending_interaction(reply_to.as_ref()).await {
                    return self.send_interaction_response(pending, text).await;
                }
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
        match body {
            OutgoingBody::Text(text) if is_interaction_response(sent) => {
                self.edit_interaction_response(sent, text).await
            }
            OutgoingBody::Text(text) => {
                let (channel_id, message_id) = parse_sent_ref(sent)?;
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

    async fn prepare_turn(&self, msg: &IncomingMessage) -> ChannelResult<ChannelTurn> {
        if self.interactions.has_pending(&msg.id).await {
            return Ok(ChannelTurn {
                reply_to: Some(msg.id.clone()),
                typing: TypingGuard::noop(),
            });
        }
        Ok(ChannelTurn {
            reply_to: Some(msg.id.clone()),
            typing: self.typing(&msg.conversation).await?,
        })
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

fn is_interaction_response(sent: &SentRef) -> bool {
    sent.raw["kind"].as_str() == Some(INTERACTION_RESPONSE_KIND)
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

fn interaction_sent_ref(channel_id: Id<ChannelMarker>, message_id: Id<MessageMarker>) -> SentRef {
    SentRef {
        channel: ID.clone(),
        message_id: MessageId(message_id.to_string()),
        raw: serde_json::json!({
            "kind": INTERACTION_RESPONSE_KIND,
            "channel_id": channel_id.to_string(),
            "message_id": message_id.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use goat_types::{ConversationId, UserHandle};

    fn handle(interactions: Arc<InteractionState>) -> DiscordHandle {
        DiscordHandle::new(
            InstanceId::new(),
            PersonaId::from_slug("dev"),
            ChannelIdentity::new("bot", "bot"),
            Arc::new(HttpClient::new("token".to_string())),
            interactions,
        )
    }

    fn message(id: &str, instance: InstanceId) -> IncomingMessage {
        IncomingMessage {
            id: MessageId(id.to_string()),
            persona: PersonaId::from_slug("dev"),
            conversation: ConversationId::new(ID.clone(), instance, "dm:456"),
            from: UserHandle {
                external: "u".into(),
                display: None,
            },
            text: "/daily-operator".into(),
            attachments: Vec::new(),
            command: None,
            ts: Utc::now(),
            raw: serde_json::json!({ "interaction_id": id }),
        }
    }

    #[tokio::test]
    async fn prepare_turn_for_pending_interaction_is_noop_typing() {
        let interactions = Arc::new(InteractionState::default());
        let instance = InstanceId::new();
        let msg = message("interaction-1", instance);
        interactions
            .insert_pending(
                msg.id.clone(),
                PendingInteraction {
                    application_id: Id::new(123),
                    token: "token".to_string(),
                    channel_id: Id::new(456),
                },
            )
            .await;

        let turn = handle(interactions).prepare_turn(&msg).await.unwrap();
        assert_eq!(turn.reply_to, Some(MessageId("interaction-1".to_string())));
        assert!(turn.typing.is_noop());
    }

    #[tokio::test]
    async fn pending_interaction_is_selected_by_reply_target() {
        let interactions = Arc::new(InteractionState::default());
        let reply_to = MessageId("interaction-1".to_string());
        interactions
            .insert_pending(
                reply_to.clone(),
                PendingInteraction {
                    application_id: Id::new(123),
                    token: "token".to_string(),
                    channel_id: Id::new(456),
                },
            )
            .await;

        let handle = handle(interactions.clone());
        let pending = handle.take_pending_interaction(Some(&reply_to)).await;
        assert!(pending.is_some());
        assert!(!interactions.has_pending(&reply_to).await);
    }

    #[test]
    fn interaction_sent_ref_marks_delivery_kind_without_token() {
        let sent = interaction_sent_ref(Id::new(456), Id::new(789));
        assert!(is_interaction_response(&sent));
        assert_eq!(sent.raw["kind"], INTERACTION_RESPONSE_KIND);
        assert!(sent.raw.get("interaction_token").is_none());
        assert!(sent.raw.get("token").is_none());
    }
}

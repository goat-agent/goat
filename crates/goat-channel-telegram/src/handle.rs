use std::time::Duration;

use async_trait::async_trait;
use goat_channel::{
    spawn_typing, ChannelCapabilities, ChannelError, ChannelHandle, ChannelIdentity, ChannelResult,
    SentRef, TypingGuard,
};
use goat_types::{ChannelId, ConversationId, InstanceId, MessageId, OutgoingBody, PersonaId};
use teloxide::prelude::*;
use teloxide::types::{ChatAction, ChatId, MessageId as TgMessageId, ReplyParameters};
use teloxide::Bot;

use crate::{CAPABILITIES, ID};

pub(crate) struct TelegramHandle {
    instance: InstanceId,
    persona: PersonaId,
    identity: ChannelIdentity,
    bot: Bot,
}

impl TelegramHandle {
    pub(crate) fn new(
        instance: InstanceId,
        persona: PersonaId,
        identity: ChannelIdentity,
        bot: Bot,
    ) -> Self {
        Self {
            instance,
            persona,
            identity,
            bot,
        }
    }
}

#[async_trait]
impl ChannelHandle for TelegramHandle {
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
        let chat_id = parse_chat_id(&conv.external)?;
        match body {
            OutgoingBody::Text(text) => {
                let mut req = self.bot.send_message(ChatId(chat_id), text);
                if let Some(rp) = parse_reply_to_message_id(reply_to.as_ref()) {
                    req = req.reply_parameters(
                        ReplyParameters::new(TgMessageId(rp)).allow_sending_without_reply(),
                    );
                }
                let sent = req
                    .send()
                    .await
                    .map_err(|e| ChannelError::Provider(e.to_string()))?;
                Ok(sent_ref(chat_id, sent.id.0))
            }
            OutgoingBody::File(_) | OutgoingBody::Reaction { .. } => {
                Err(ChannelError::Unsupported("telegram: v0 only supports text"))
            }
            _ => Err(ChannelError::Unsupported("telegram: unknown outgoing body")),
        }
    }

    async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()> {
        let (chat_id, message_id) = parse_sent_ref(sent)?;
        match body {
            OutgoingBody::Text(text) => {
                self.bot
                    .edit_message_text(ChatId(chat_id), TgMessageId(message_id), text)
                    .send()
                    .await
                    .map_err(|e| ChannelError::Provider(e.to_string()))?;
                Ok(())
            }
            _ => Err(ChannelError::Unsupported(
                "telegram: edit only supports text",
            )),
        }
    }

    async fn typing(&self, conv: &ConversationId) -> ChannelResult<TypingGuard> {
        let chat_id = parse_chat_id(&conv.external)?;
        let bot = self.bot.clone();
        let refresh = CAPABILITIES
            .typing_refresh
            .unwrap_or(Duration::from_secs(4));
        Ok(spawn_typing(refresh, move || {
            let bot = bot.clone();
            async move {
                let _ = bot
                    .send_chat_action(ChatId(chat_id), ChatAction::Typing)
                    .send()
                    .await;
            }
        }))
    }
}

fn parse_chat_id(s: &str) -> ChannelResult<i64> {
    s.strip_prefix("chat:")
        .ok_or_else(|| ChannelError::BadRequest(format!("not a telegram conversation: {s}")))?
        .parse::<i64>()
        .map_err(|e| ChannelError::BadRequest(format!("bad chat_id: {e}")))
}

fn parse_sent_ref(sent: &SentRef) -> ChannelResult<(i64, i32)> {
    let chat_id = sent.raw["chat_id"]
        .as_i64()
        .ok_or_else(|| ChannelError::BadRequest("missing chat_id in sent ref".into()))?;
    let message_id = sent.raw["message_id"]
        .as_i64()
        .ok_or_else(|| ChannelError::BadRequest("missing message_id in sent ref".into()))?
        as i32;
    Ok((chat_id, message_id))
}

fn sent_ref(chat_id: i64, message_id: i32) -> SentRef {
    SentRef {
        channel: ID.clone(),
        message_id: MessageId(message_id.to_string()),
        raw: serde_json::json!({ "chat_id": chat_id, "message_id": message_id }),
    }
}

/// Parses a `MessageId` into a Telegram message id (`i32`).
/// Returns `None` if `reply_to` is `None` or the inner string is not a valid
/// `i32`, and logs a debug message in the latter case.
fn parse_reply_to_message_id(reply_to: Option<&MessageId>) -> Option<i32> {
    let id = reply_to?;
    match id.0.parse::<i32>() {
        Ok(n) => Some(n),
        Err(e) => {
            tracing::debug!("telegram: ignoring unparseable reply_to {:?}: {e}", id.0);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reply_to_some_valid() {
        assert_eq!(
            parse_reply_to_message_id(Some(&MessageId("42".into()))),
            Some(42)
        );
    }

    #[test]
    fn parse_reply_to_some_invalid() {
        assert_eq!(
            parse_reply_to_message_id(Some(&MessageId("bad".into()))),
            None
        );
    }

    #[test]
    fn parse_reply_to_none() {
        assert_eq!(parse_reply_to_message_id(None), None);
    }
}

use std::time::Duration;

use chrono::{DateTime, Utc};
use goat_types::{
    Attachment, AttachmentSource, ConversationId, IncomingMessage, InstanceId, MessageId,
    PersonaId, UserHandle,
};
use teloxide::prelude::*;
use teloxide::types::UpdateKind;
use teloxide::Bot;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::ID;

pub(crate) async fn poll_loop(
    bot: Bot,
    persona: PersonaId,
    instance: InstanceId,
    tx: mpsc::Sender<IncomingMessage>,
) {
    let mut offset: i32 = 0;
    loop {
        match bot.get_updates().offset(offset).timeout(30).send().await {
            Ok(updates) => {
                for update in updates {
                    let next = update.id.0 as i32 + 1;
                    if next > offset {
                        offset = next;
                    }
                    if let Some(msg) = update_to_incoming(persona, instance, update) {
                        if tx.send(msg).await.is_err() {
                            warn!("telegram receiver dropped; stopping poll");
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                warn!(error = ?e, "telegram poll error; backing off");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

fn update_to_incoming(
    persona: PersonaId,
    instance: InstanceId,
    update: teloxide::types::Update,
) -> Option<IncomingMessage> {
    let UpdateKind::Message(message) = update.kind else {
        return None;
    };
    let from = message.from.as_ref()?;
    let attachments = extract_attachments(&message);
    let text = message
        .text()
        .or_else(|| message.caption())
        .unwrap_or("")
        .to_string();
    if text.is_empty() && attachments.is_empty() {
        return None;
    }
    let conv = ConversationId::new(ID.clone(), instance, format!("chat:{}", message.chat.id.0));
    let ts: DateTime<Utc> = message.date;
    debug!(text = %text, attachments = attachments.len(), chat = ?message.chat.id, "telegram update");
    Some(IncomingMessage {
        id: MessageId(message.id.0.to_string()),
        persona,
        conversation: conv,
        from: UserHandle {
            external: from.id.0.to_string(),
            display: Some(from.full_name()),
        },
        text,
        attachments,
        ts,
        raw: serde_json::json!({ "chat_id": message.chat.id.0, "message_id": message.id.0 }),
    })
}

fn extract_attachments(message: &teloxide::types::Message) -> Vec<Attachment> {
    let mut atts = Vec::new();
    if let Some(photos) = message.photo() {
        if let Some(largest) = photos
            .iter()
            .max_by_key(|p| p.width.saturating_mul(p.height))
        {
            atts.push(Attachment {
                mime: "image/jpeg".to_string(),
                name: None,
                size: largest.file.size as u64,
                source: file_ref(largest.file.id.to_string()),
            });
        }
    }
    if let Some(doc) = message.document() {
        atts.push(Attachment {
            mime: doc
                .mime_type
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".into()),
            name: doc.file_name.clone(),
            size: doc.file.size as u64,
            source: file_ref(doc.file.id.to_string()),
        });
    }
    if let Some(voice) = message.voice() {
        atts.push(Attachment {
            mime: voice
                .mime_type
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "audio/ogg".into()),
            name: None,
            size: voice.file.size as u64,
            source: file_ref(voice.file.id.to_string()),
        });
    }
    if let Some(audio) = message.audio() {
        atts.push(Attachment {
            mime: audio
                .mime_type
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "audio/mpeg".into()),
            name: audio.title.clone(),
            size: audio.file.size as u64,
            source: file_ref(audio.file.id.to_string()),
        });
    }
    if let Some(video) = message.video() {
        atts.push(Attachment {
            mime: video
                .mime_type
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "video/mp4".into()),
            name: video.file_name.clone(),
            size: video.file.size as u64,
            source: file_ref(video.file.id.to_string()),
        });
    }
    atts
}

fn file_ref(value: String) -> AttachmentSource {
    AttachmentSource::ChannelRef {
        channel: ID.clone(),
        kind: "file_id".to_string(),
        value,
        raw: serde_json::Value::Null,
    }
}

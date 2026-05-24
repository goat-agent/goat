use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use goat_command::CommandSpec;
use goat_types::{
    Attachment, AttachmentSource, CommandCall, CommandName, ConversationId, IncomingMessage,
    InstanceId, MessageId, PersonaId, UserHandle,
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
    commands: Vec<CommandSpec>,
    allowed_user_ids: HashSet<i64>,
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
                    if let Some(msg) =
                        update_to_incoming(persona, instance, update, &commands, &allowed_user_ids)
                    {
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
    commands: &[CommandSpec],
    allowed_user_ids: &HashSet<i64>,
) -> Option<IncomingMessage> {
    let UpdateKind::Message(message) = update.kind else {
        return None;
    };
    let from = message.from.as_ref()?;
    if !is_allowed_user_id(from.id.0, allowed_user_ids) {
        debug!(
            user_id = from.id.0,
            "telegram update rejected by allowed_user_ids"
        );
        return None;
    }
    let attachments = extract_attachments(&message);
    let raw_text = message
        .text()
        .or_else(|| message.caption())
        .unwrap_or("")
        .to_string();
    let command = parse_command(&raw_text, &message.id.0.to_string(), commands);
    let text = command.as_ref().map(command_text).unwrap_or(raw_text);
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
        command,
        ts,
        raw: serde_json::json!({ "chat_id": message.chat.id.0, "message_id": message.id.0 }),
    })
}

pub(crate) fn telegram_command_name(skill_name: &str) -> Option<String> {
    let mut out = String::new();
    for ch in skill_name.chars() {
        match ch {
            'a'..='z' | '0'..='9' | '_' => out.push(ch),
            'A'..='Z' => out.push(ch.to_ascii_lowercase()),
            '-' | ' ' => out.push('_'),
            _ => {}
        }
        if out.len() >= 32 {
            break;
        }
    }
    (!out.is_empty()
        && out
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
    .then_some(out)
}

fn parse_command(text: &str, call_id: &str, commands: &[CommandSpec]) -> Option<CommandCall> {
    let rest = text.strip_prefix('/')?;
    let (head, args) = split_command(rest);
    let platform_name = head.split_once('@').map(|(cmd, _)| cmd).unwrap_or(head);
    let spec = commands.iter().find(|command| {
        telegram_command_name(command.name.as_str()).as_deref() == Some(platform_name)
    })?;
    Some(CommandCall::new(
        call_id.to_string(),
        CommandName::new(spec.name.as_str().to_string()).ok()?,
        args.to_string(),
        serde_json::json!({ "platform": "telegram", "command": platform_name }),
    ))
}

fn command_text(call: &CommandCall) -> String {
    if call.args.is_empty() {
        format!("/{}", call.name.as_str())
    } else {
        format!("/{} {}", call.name.as_str(), call.args)
    }
}

fn is_allowed_user_id(user_id: u64, allowed_user_ids: &HashSet<i64>) -> bool {
    allowed_user_ids.is_empty()
        || i64::try_from(user_id)
            .ok()
            .is_some_and(|id| allowed_user_ids.contains(&id))
}

fn split_command(rest: &str) -> (&str, &str) {
    let index = rest
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(i, _)| i);
    match index {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(values: &[i64]) -> HashSet<i64> {
        values.iter().copied().collect()
    }

    #[test]
    fn allowlist_empty_allows_any_user() {
        assert!(is_allowed_user_id(42, &allowlist(&[])));
    }

    #[test]
    fn allowlist_accepts_configured_user() {
        assert!(is_allowed_user_id(42, &allowlist(&[42])));
    }

    #[test]
    fn allowlist_rejects_unconfigured_user() {
        assert!(!is_allowed_user_id(7, &allowlist(&[42])));
    }
}

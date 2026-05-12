use chrono::Utc;
use goat_types::{
    Attachment, AttachmentSource, ConversationId, IncomingMessage, InstanceId, MessageId,
    PersonaId, UserHandle,
};
use tokio::sync::mpsc;
use tracing::warn;
use twilight_gateway::{EventTypeFlags, Shard, StreamExt as _GatewayStreamExt};
use twilight_model::gateway::event::Event;

use crate::ID;

pub(crate) async fn gateway_loop(
    mut shard: Shard,
    persona: PersonaId,
    instance: InstanceId,
    tx: mpsc::Sender<IncomingMessage>,
) {
    loop {
        let Some(item) = shard.next_event(EventTypeFlags::MESSAGE_CREATE).await else {
            return;
        };
        match item {
            Ok(Event::MessageCreate(mc)) => {
                if mc.author.bot {
                    continue;
                }
                let external = match mc.guild_id {
                    Some(g) => format!("g:{}:c:{}", g, mc.channel_id),
                    None => format!("dm:{}", mc.channel_id),
                };
                let conv = ConversationId::new(ID.clone(), instance, external);
                let attachments: Vec<Attachment> = mc
                    .attachments
                    .iter()
                    .map(|a| Attachment {
                        mime: a
                            .content_type
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".into()),
                        name: Some(a.filename.clone()),
                        size: a.size,
                        source: AttachmentSource::Url(a.url.clone()),
                    })
                    .collect();
                let msg = IncomingMessage {
                    id: MessageId(mc.id.to_string()),
                    persona,
                    conversation: conv,
                    from: UserHandle {
                        external: mc.author.id.to_string(),
                        display: Some(mc.author.name.clone()),
                    },
                    text: mc.content.clone(),
                    attachments,
                    ts: Utc::now(),
                    raw: serde_json::json!({
                        "channel_id": mc.channel_id.to_string(),
                        "message_id": mc.id.to_string(),
                        "guild_id": mc.guild_id.map(|g| g.to_string()),
                    }),
                };
                if tx.send(msg).await.is_err() {
                    warn!("discord receiver dropped");
                    return;
                }
            }
            Ok(_) => {}
            Err(e) => warn!(error = ?e, "discord gateway error"),
        }
    }
}

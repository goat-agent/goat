use std::sync::Arc;

use chrono::Utc;
use goat_command::{CommandCall, CommandName, CommandSpec};
use goat_types::{
    Attachment, AttachmentSource, ConversationId, IncomingMessage, InstanceId, MessageId,
    PersonaId, UserHandle,
};
use tokio::sync::mpsc;
use tracing::warn;
use twilight_gateway::{EventTypeFlags, Shard, StreamExt as _GatewayStreamExt};
use twilight_http::Client as HttpClient;
use twilight_model::application::interaction::{
    application_command::CommandOptionValue, InteractionData,
};
use twilight_model::gateway::event::Event;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};

use crate::ID;

pub(crate) async fn gateway_loop(
    mut shard: Shard,
    http: Arc<HttpClient>,
    persona: PersonaId,
    instance: InstanceId,
    tx: mpsc::Sender<IncomingMessage>,
    commands: Vec<CommandSpec>,
) {
    let events = EventTypeFlags::MESSAGE_CREATE | EventTypeFlags::INTERACTION_CREATE;
    loop {
        let Some(item) = shard.next_event(events).await else {
            return;
        };
        match item {
            Ok(Event::MessageCreate(mc)) => {
                if mc.author.bot {
                    continue;
                }
                let command = parse_text_command(&mc.content, &mc.id.to_string(), &commands);
                let text = command
                    .as_ref()
                    .map(command_text)
                    .unwrap_or_else(|| mc.content.clone());
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
                    text,
                    attachments,
                    command,
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
            Ok(Event::InteractionCreate(ic)) => {
                let Some(msg) = interaction_to_incoming(&ic, persona, instance, &commands) else {
                    continue;
                };
                acknowledge_interaction(http.clone(), &ic).await;
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

async fn acknowledge_interaction(
    http: Arc<HttpClient>,
    interaction: &twilight_model::gateway::payload::incoming::InteractionCreate,
) {
    let response = InteractionResponse {
        kind: InteractionResponseType::DeferredChannelMessageWithSource,
        data: None,
    };
    if let Err(e) = http
        .interaction(interaction.application_id)
        .create_response(interaction.id, &interaction.token, &response)
        .await
    {
        warn!(error = ?e, "discord interaction acknowledgement failed");
    }
}

fn interaction_to_incoming(
    interaction: &twilight_model::gateway::payload::incoming::InteractionCreate,
    persona: PersonaId,
    instance: InstanceId,
    commands: &[CommandSpec],
) -> Option<IncomingMessage> {
    let data = match interaction.data.as_ref()? {
        InteractionData::ApplicationCommand(data) => data,
        _ => return None,
    };
    let spec = commands.iter().find(|command| {
        discord_command_name(command.name.as_str()).as_deref() == Some(data.name.as_str())
    })?;
    let args = data
        .options
        .iter()
        .find(|option| option.name == "args")
        .and_then(|option| match &option.value {
            CommandOptionValue::String(value) => Some(value.as_str()),
            _ => None,
        })
        .unwrap_or("");
    #[allow(deprecated)]
    let channel_id = interaction.channel_id?;
    let external = match interaction.guild_id {
        Some(guild_id) => format!("g:{}:c:{}", guild_id, channel_id),
        None => format!("dm:{}", channel_id),
    };
    let author = interaction.author()?;
    Some(IncomingMessage {
        id: MessageId(interaction.id.to_string()),
        persona,
        conversation: ConversationId::new(ID.clone(), instance, external),
        from: UserHandle {
            external: author.id.to_string(),
            display: Some(author.name.clone()),
        },
        text: command_text(&CommandCall::new(
            interaction.id.to_string(),
            CommandName::new(spec.name.as_str().to_string()).ok()?,
            args.to_string(),
            serde_json::json!({ "platform": "discord", "command": data.name }),
        )),
        attachments: Vec::new(),
        command: Some(CommandCall::new(
            interaction.id.to_string(),
            CommandName::new(spec.name.as_str().to_string()).ok()?,
            args.to_string(),
            serde_json::json!({ "platform": "discord", "command": data.name }),
        )),
        ts: Utc::now(),
        raw: serde_json::json!({
            "interaction_id": interaction.id.to_string(),
            "channel_id": channel_id.to_string(),
            "guild_id": interaction.guild_id.map(|g| g.to_string()),
        }),
    })
}

pub(crate) fn discord_command_name(skill_name: &str) -> Option<String> {
    let mut out = String::new();
    for ch in skill_name.chars() {
        match ch {
            'a'..='z' | '0'..='9' | '_' | '-' => out.push(ch),
            'A'..='Z' => out.push(ch.to_ascii_lowercase()),
            ' ' => out.push('-'),
            _ => {}
        }
        if out.len() >= 32 {
            break;
        }
    }
    (!out.is_empty()
        && out
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'))
    .then_some(out)
}

fn parse_text_command(text: &str, call_id: &str, commands: &[CommandSpec]) -> Option<CommandCall> {
    let rest = text.strip_prefix('/')?;
    let (head, args) = split_command(rest);
    let spec = commands
        .iter()
        .find(|command| discord_command_name(command.name.as_str()).as_deref() == Some(head))?;
    Some(CommandCall::new(
        call_id.to_string(),
        CommandName::new(spec.name.as_str().to_string()).ok()?,
        args.to_string(),
        serde_json::json!({ "platform": "discord", "command": head }),
    ))
}

fn command_text(call: &CommandCall) -> String {
    if call.args.is_empty() {
        format!("/{}", call.name.as_str())
    } else {
        format!("/{} {}", call.name.as_str(), call.args)
    }
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

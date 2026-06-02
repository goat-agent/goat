use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use goat_command::CommandSpec;
use goat_types::{
    Attachment, AttachmentSource, CommandCall, CommandName, ConversationId, IncomingMessage,
    InstanceId, MessageId, PersonaId, UserHandle,
};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use twilight_gateway::{EventTypeFlags, Intents, Shard, ShardId, StreamExt as _GatewayStreamExt};
use twilight_http::Client as HttpClient;
use twilight_model::application::interaction::{
    application_command::CommandOptionValue, InteractionData,
};
use twilight_model::gateway::event::Event;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};

use crate::interaction::{InteractionState, PendingInteraction};
use crate::ID;

pub(crate) struct GatewayConfig {
    pub(crate) persona: PersonaId,
    pub(crate) instance: InstanceId,
    pub(crate) commands: Vec<CommandSpec>,
    pub(crate) interactions: Arc<InteractionState>,
    pub(crate) allowed_user_ids: HashSet<u64>,
    pub(crate) token: String,
    pub(crate) intents: Intents,
}

pub(crate) async fn gateway_loop(
    http: Arc<HttpClient>,
    tx: mpsc::Sender<IncomingMessage>,
    cfg: GatewayConfig,
) {
    let GatewayConfig {
        persona,
        instance,
        commands,
        interactions,
        allowed_user_ids,
        token,
        intents,
    } = cfg;
    let events = EventTypeFlags::MESSAGE_CREATE | EventTypeFlags::INTERACTION_CREATE;
    let mut backoff_secs: u64 = 1;
    'reconnect: loop {
        let mut shard = Shard::new(ShardId::ONE, token.clone(), intents);
        loop {
            match shard.next_event(events).await {
                None => break, // fatal stream close → reconnect
                Some(Ok(Event::MessageCreate(mc))) => {
                    backoff_secs = 1;
                    if mc.author.bot {
                        continue;
                    }
                    if !is_allowed_user_id(mc.author.id.get(), &allowed_user_ids) {
                        debug!(user_id = mc.author.id.get(), "discord: user not in allowlist, ignoring");
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
                        break 'reconnect;
                    }
                }
                Some(Ok(Event::InteractionCreate(ic))) => {
                    backoff_secs = 1;
                    match ic.author() {
                        Some(author) if !is_allowed_user_id(author.id.get(), &allowed_user_ids) => {
                            debug!(
                                user_id = author.id.get(),
                                "discord: interaction user not in allowlist, ignoring"
                            );
                            continue;
                        }
                        None if !allowed_user_ids.is_empty() => {
                            debug!("discord: interaction with no author and allowlist active, ignoring");
                            continue;
                        }
                        _ => {}
                    }
                    let Some((msg, pending)) =
                        interaction_to_incoming(&ic, persona, instance, &commands)
                    else {
                        continue;
                    };
                    if !acknowledge_interaction(http.clone(), &ic).await {
                        continue;
                    }
                    interactions.insert_pending(msg.id.clone(), pending).await;
                    if tx.send(msg).await.is_err() {
                        warn!("discord receiver dropped");
                        break 'reconnect;
                    }
                }
                Some(Ok(_)) => {
                    backoff_secs = 1;
                }
                Some(Err(e)) => warn!(error = ?e, "discord gateway error"),
            }
        }
        warn!(backoff_secs, "discord gateway closed; reconnecting");
        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

async fn acknowledge_interaction(
    http: Arc<HttpClient>,
    interaction: &twilight_model::gateway::payload::incoming::InteractionCreate,
) -> bool {
    let response = InteractionResponse {
        kind: InteractionResponseType::DeferredChannelMessageWithSource,
        data: None,
    };
    match http
        .interaction(interaction.application_id)
        .create_response(interaction.id, &interaction.token, &response)
        .await
    {
        Ok(_) => true,
        Err(e) => {
            warn!(error = ?e, "discord interaction acknowledgement failed");
            false
        }
    }
}

fn interaction_to_incoming(
    interaction: &twilight_model::gateway::payload::incoming::InteractionCreate,
    persona: PersonaId,
    instance: InstanceId,
    commands: &[CommandSpec],
) -> Option<(IncomingMessage, PendingInteraction)> {
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
    let pending = PendingInteraction {
        application_id: interaction.application_id,
        token: interaction.token.clone(),
        channel_id,
    };
    Some((
        IncomingMessage {
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
        },
        pending,
    ))
}

fn is_allowed_user_id(user_id: u64, allowed_user_ids: &HashSet<u64>) -> bool {
    allowed_user_ids.is_empty() || allowed_user_ids.contains(&user_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(values: &[u64]) -> HashSet<u64> {
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

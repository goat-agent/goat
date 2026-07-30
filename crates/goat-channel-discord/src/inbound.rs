use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use goat_agent_command::{CommandArgs, CommandSpec};
use goat_types::{
    Attachment, AttachmentSource, CommandCall, CommandName, IncomingMessage, InstanceId, MessageId,
    ProfileId, Surface, ThreadId, UserHandle,
};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use twilight_gateway::{EventTypeFlags, Intents, Shard, ShardId, StreamExt as _GatewayStreamExt};
use twilight_http::Client as HttpClient;
use twilight_model::application::interaction::{
    InteractionData, application_command::CommandOptionValue,
};
use twilight_model::gateway::event::Event;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, GuildMarker};

type ChannelMetaCache = HashMap<u64, (bool, Option<u64>)>;

use crate::ID;
use crate::interaction::{InteractionState, PendingInteraction};

pub(crate) struct GatewayConfig {
    pub(crate) persona: ProfileId,
    pub(crate) instance: InstanceId,
    pub(crate) commands: Vec<CommandSpec>,
    pub(crate) interactions: Arc<InteractionState>,
    pub(crate) allowed_user_ids: HashSet<u64>,
    pub(crate) bot_id: u64,
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
        bot_id,
        token,
        intents,
    } = cfg;
    let events = EventTypeFlags::MESSAGE_CREATE | EventTypeFlags::INTERACTION_CREATE;
    let mut channel_cache: ChannelMetaCache = HashMap::new();
    let mut backoff_secs: u64 = 1;
    'reconnect: loop {
        let mut shard = Shard::new(ShardId::ONE, token.clone(), intents);
        loop {
            match shard.next_event(events).await {
                None => break,
                Some(Ok(Event::MessageCreate(mc))) => {
                    backoff_secs = 1;
                    if mc.author.bot {
                        continue;
                    }
                    if !is_allowed_user_id(mc.author.id.get(), &allowed_user_ids) {
                        debug!(
                            user_id = mc.author.id.get(),
                            "discord: user not in allowlist, ignoring"
                        );
                        continue;
                    }
                    let stripped = strip_bot_mention(&mc.content, bot_id);
                    let command = parse_text_command(&stripped, &mc.id.to_string(), &commands);
                    let text = command
                        .as_ref()
                        .map_or_else(|| stripped.clone(), command_text);
                    let external = match mc.guild_id {
                        Some(g) => format!("g:{}:c:{}", g, mc.channel_id),
                        None => format!("dm:{}", mc.channel_id),
                    };
                    let conv = ThreadId::new(ID.clone(), instance, external);
                    let (surface, parent) =
                        classify_surface(&http, &mut channel_cache, mc.guild_id, mc.channel_id)
                            .await;
                    let mention_ids: Vec<u64> = mc.mentions.iter().map(|m| m.id.get()).collect();
                    let referenced_author =
                        mc.referenced_message.as_ref().map(|r| r.author.id.get());
                    let addressed = is_addressed(&mention_ids, referenced_author, bot_id);
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
                        profile: persona,
                        thread: conv,
                        from: UserHandle {
                            external: mc.author.id.to_string(),
                            display: Some(mc.author.name.clone()),
                        },
                        text,
                        attachments,
                        command,
                        surface,
                        addressed,
                        parent,
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
                            debug!(
                                "discord: interaction with no author and allowlist active, ignoring"
                            );
                            continue;
                        }
                        _ => {}
                    }
                    let Some((msg, pending)) = interaction_to_incoming(
                        &http,
                        &mut channel_cache,
                        &ic,
                        persona,
                        instance,
                        &commands,
                    )
                    .await
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
                Some(Err(e)) => {
                    backoff_secs = 1;
                    warn!(error = ?e, "discord gateway error");
                }
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

async fn interaction_to_incoming(
    http: &HttpClient,
    cache: &mut ChannelMetaCache,
    interaction: &twilight_model::gateway::payload::incoming::InteractionCreate,
    persona: ProfileId,
    instance: InstanceId,
    commands: &[CommandSpec],
) -> Option<(IncomingMessage, PendingInteraction)> {
    let InteractionData::ApplicationCommand(data) = interaction.data.as_ref()? else {
        return None;
    };
    let spec = commands.iter().find(|command| {
        discord_command_name(command.name.as_str()).as_deref() == Some(data.name.as_str())
    })?;
    let option_value = |name: &str| {
        data.options
            .iter()
            .find(|option| option.name == name)
            .and_then(|option| match &option.value {
                CommandOptionValue::String(value) => Some(value.as_str()),
                _ => None,
            })
    };
    let (args, named) = match &spec.args {
        CommandArgs::Named(specs) => {
            let values: Vec<(&str, &str)> = specs
                .iter()
                .filter_map(|arg| option_value(&arg.name).map(|value| (arg.name.as_str(), value)))
                .collect();
            let joined = values
                .iter()
                .map(|(_, value)| quote_arg(value))
                .collect::<Vec<_>>()
                .join(" ");
            let map: serde_json::Map<String, serde_json::Value> = values
                .into_iter()
                .map(|(name, value)| (name.to_string(), serde_json::Value::from(value)))
                .collect();
            (joined, Some(serde_json::Value::Object(map)))
        }
        _ => (option_value("args").unwrap_or("").to_string(), None),
    };
    let raw_command = match &named {
        Some(arguments) => {
            serde_json::json!({ "platform": "discord", "command": data.name, "arguments": arguments })
        }
        None => serde_json::json!({ "platform": "discord", "command": data.name }),
    };
    #[allow(deprecated)]
    let channel_id = interaction.channel_id?;
    let external = match interaction.guild_id {
        Some(guild_id) => format!("g:{guild_id}:c:{channel_id}"),
        None => format!("dm:{channel_id}"),
    };
    let (surface, parent) = classify_surface(http, cache, interaction.guild_id, channel_id).await;
    let author = interaction.author()?;
    let pending = PendingInteraction {
        application_id: interaction.application_id,
        token: interaction.token.clone(),
        channel_id,
    };
    Some((
        IncomingMessage {
            id: MessageId(interaction.id.to_string()),
            profile: persona,
            thread: ThreadId::new(ID.clone(), instance, external),
            from: UserHandle {
                external: author.id.to_string(),
                display: Some(author.name.clone()),
            },
            text: command_text(&CommandCall::new(
                interaction.id.to_string(),
                CommandName::new(spec.name.as_str().to_string()).ok()?,
                args.clone(),
                raw_command.clone(),
            )),
            attachments: Vec::new(),
            command: Some(CommandCall::new(
                interaction.id.to_string(),
                CommandName::new(spec.name.as_str().to_string()).ok()?,
                args,
                raw_command,
            )),
            surface,
            addressed: true,
            parent,
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

async fn channel_meta(
    http: &HttpClient,
    cache: &mut ChannelMetaCache,
    channel_id: Id<ChannelMarker>,
) -> (bool, Option<u64>) {
    let key = channel_id.get();
    if let Some(hit) = cache.get(&key) {
        return *hit;
    }
    let meta = match http.channel(channel_id).await {
        Ok(response) => match response.model().await {
            Ok(channel) => (channel.kind.is_thread(), channel.parent_id.map(Id::get)),
            Err(e) => {
                warn!(error = ?e, channel_id = key, "discord: channel decode failed");
                (false, None)
            }
        },
        Err(e) => {
            warn!(error = ?e, channel_id = key, "discord: channel lookup failed");
            (false, None)
        }
    };
    cache.insert(key, meta);
    meta
}

async fn classify_surface(
    http: &HttpClient,
    cache: &mut ChannelMetaCache,
    guild_id: Option<Id<GuildMarker>>,
    channel_id: Id<ChannelMarker>,
) -> (Surface, Option<String>) {
    let Some(guild) = guild_id else {
        return (Surface::Dm, None);
    };
    let (is_thread, parent_id) = channel_meta(http, cache, channel_id).await;
    if is_thread {
        (
            Surface::Thread,
            parent_id.map(|parent| format!("g:{guild}:c:{parent}")),
        )
    } else {
        (Surface::Channel, None)
    }
}

fn is_addressed(mention_ids: &[u64], referenced_author_id: Option<u64>, bot_id: u64) -> bool {
    mention_ids.contains(&bot_id) || referenced_author_id == Some(bot_id)
}

fn strip_bot_mention(content: &str, bot_id: u64) -> String {
    content
        .replace(&format!("<@{bot_id}>"), "")
        .replace(&format!("<@!{bot_id}>"), "")
        .trim()
        .to_string()
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

fn quote_arg(value: &str) -> String {
    if value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
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

    #[test]
    fn addressed_by_direct_mention() {
        assert!(is_addressed(&[10, 42], None, 42));
    }

    #[test]
    fn addressed_by_reply_to_bot() {
        assert!(is_addressed(&[], Some(42), 42));
    }

    #[test]
    fn not_addressed_without_mention_or_reply() {
        assert!(!is_addressed(&[10, 11], Some(7), 42));
        assert!(!is_addressed(&[], None, 42));
    }

    #[test]
    fn strip_bot_mention_removes_both_forms_and_trims() {
        assert_eq!(strip_bot_mention("<@42> hello", 42), "hello");
        assert_eq!(strip_bot_mention("<@!42> hi there", 42), "hi there");
        assert_eq!(strip_bot_mention("  <@42>  ", 42), "");
        assert_eq!(strip_bot_mention("no mention here", 42), "no mention here");
    }
}

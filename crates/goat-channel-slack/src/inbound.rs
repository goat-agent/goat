use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::{SinkExt, StreamExt};
use goat_agent_command::CommandSpec;
use goat_types::{
    CommandCall, CommandName, IncomingMessage, InstanceId, MessageId, ProfileId, Surface, ThreadId,
    UserHandle,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

use crate::ID;
use crate::api::SlackApi;
use crate::socket::{self, Incoming};
use crate::{mrkdwn, thread};

const MAX_BACKOFF_SECS: u64 = 60;

pub(crate) struct SocketConfig {
    pub(crate) persona: ProfileId,
    pub(crate) instance: InstanceId,
    pub(crate) commands: Vec<CommandSpec>,
    pub(crate) allowed_user_ids: HashSet<String>,
    pub(crate) bot_user_id: String,
    pub(crate) bot_id: Option<String>,
    pub(crate) app_token: String,
    pub(crate) api: Arc<SlackApi>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MessageEvent {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) channel: String,
    #[serde(default)]
    pub(crate) user: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) ts: String,
    #[serde(default)]
    pub(crate) thread_ts: Option<String>,
    #[serde(default)]
    pub(crate) parent_user_id: Option<String>,
    #[serde(default)]
    pub(crate) channel_type: Option<String>,
    #[serde(default)]
    pub(crate) bot_id: Option<String>,
    #[serde(default)]
    pub(crate) subtype: Option<String>,
}

pub(crate) async fn socket_loop(cfg: SocketConfig, tx: mpsc::Sender<IncomingMessage>) {
    let SocketConfig {
        persona,
        instance,
        commands,
        allowed_user_ids,
        bot_user_id,
        bot_id,
        app_token,
        api,
    } = cfg;
    let mut names: HashMap<String, String> = HashMap::new();
    let mut backoff_secs: u64 = 1;

    'reconnect: loop {
        if tx.is_closed() {
            break;
        }
        let url = match crate::api::open_connection(&app_token).await {
            Ok(url) => url,
            Err(e) => {
                warn!(profile = %persona, error = ?e, "slack: could not open a socket mode connection");
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                continue;
            }
        };
        let mut stream = match tokio_tungstenite::connect_async(&url).await {
            Ok((stream, _)) => stream,
            Err(e) => {
                warn!(profile = %persona, error = ?e, "slack: socket mode handshake failed");
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                continue;
            }
        };
        info!(profile = %persona, "slack: socket mode connected");

        while let Some(frame) = stream.next().await {
            let raw = match frame {
                Ok(WsMessage::Text(text)) => text.to_string(),
                Ok(WsMessage::Close(_)) => break,
                Ok(_) => continue,
                Err(e) => {
                    warn!(profile = %persona, error = ?e, "slack: socket read failed");
                    break;
                }
            };
            backoff_secs = 1;

            let classified = socket::classify(&raw);
            if let Some(envelope_id) = envelope_of(&classified)
                && let Err(e) = stream.send(WsMessage::text(socket::ack(envelope_id))).await
            {
                warn!(profile = %persona, error = ?e, "slack: ack failed");
                break;
            }

            match classified {
                Incoming::Hello => debug!(profile = %persona, "slack: hello"),
                Incoming::Disconnect { reason } => {
                    debug!(profile = %persona, %reason, "slack: server asked us to reconnect");
                    break;
                }
                Incoming::Ignored { kind, .. } => {
                    debug!(profile = %persona, %kind, "slack: ignoring frame");
                }
                Incoming::Unparsable => {
                    warn!(profile = %persona, "slack: could not parse a socket frame");
                }
                Incoming::Event { event, .. } => {
                    let Ok(event) = serde_json::from_value::<MessageEvent>(event) else {
                        debug!(profile = %persona, "slack: ignoring an event with an unexpected shape");
                        continue;
                    };
                    if !should_handle(&event, &bot_user_id, bot_id.as_deref(), &allowed_user_ids) {
                        continue;
                    }
                    let display = resolve_display(&api, &mut names, event.user.as_deref()).await;
                    let Some(message) =
                        to_incoming(&event, persona, instance, &bot_user_id, &commands, display)
                    else {
                        continue;
                    };
                    if tx.send(message).await.is_err() {
                        break 'reconnect;
                    }
                }
            }
        }

        if tx.is_closed() {
            break;
        }
        warn!(profile = %persona, backoff_secs, "slack: socket closed; reconnecting");
        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
    }
    info!(profile = %persona, "slack: socket mode loop stopped");
}

fn envelope_of(incoming: &Incoming) -> Option<&str> {
    match incoming {
        Incoming::Event { envelope_id, .. } => Some(envelope_id),
        Incoming::Ignored { envelope_id, .. } => envelope_id.as_deref(),
        Incoming::Hello | Incoming::Disconnect { .. } | Incoming::Unparsable => None,
    }
}

async fn resolve_display(
    api: &SlackApi,
    names: &mut HashMap<String, String>,
    user: Option<&str>,
) -> Option<String> {
    let user = user?;
    if let Some(known) = names.get(user) {
        return Some(known.clone());
    }
    match api.user_profile(user).await {
        Ok(profile) => {
            names.insert(user.to_string(), profile.display.clone());
            Some(profile.display)
        }
        Err(e) => {
            debug!(user, error = ?e, "slack: could not resolve a display name");
            None
        }
    }
}

fn should_handle(
    event: &MessageEvent,
    bot_user_id: &str,
    bot_id: Option<&str>,
    allowed_user_ids: &HashSet<String>,
) -> bool {
    if event.kind != "message" {
        return false;
    }
    if event.subtype.is_some() {
        return false;
    }
    if event.channel.is_empty() || event.ts.is_empty() {
        return false;
    }
    if event.bot_id.is_some() && event.bot_id.as_deref() == bot_id {
        return false;
    }
    let Some(user) = event.user.as_deref() else {
        return false;
    };
    if user == bot_user_id {
        return false;
    }
    allowed_user_ids.is_empty() || allowed_user_ids.contains(user)
}

fn to_incoming(
    event: &MessageEvent,
    persona: ProfileId,
    instance: InstanceId,
    bot_user_id: &str,
    commands: &[CommandSpec],
    display: Option<String>,
) -> Option<IncomingMessage> {
    let user = event.user.clone()?;
    let raw_text = event.text.clone().unwrap_or_default();
    let in_thread = thread::is_thread_reply(event.thread_ts.as_deref(), &event.ts);
    let surface = thread::surface_of(event.channel_type.as_deref(), &event.channel, in_thread);

    let anchor = if in_thread {
        event.thread_ts.as_deref()
    } else {
        None
    };
    let external = thread::external(&event.channel, anchor);
    let parent = in_thread.then(|| thread::external(&event.channel, None));

    let addressed = mrkdwn::mentions(&raw_text, bot_user_id)
        || event.parent_user_id.as_deref() == Some(bot_user_id)
        || surface == Surface::Dm;

    let text = mrkdwn::from_mrkdwn(&mrkdwn::strip_mention(&raw_text, bot_user_id));
    let command = parse_text_command(&text, &event.ts, commands);
    let text = match command.as_ref() {
        Some(call) => command_text(call),
        None => text,
    };

    Some(IncomingMessage {
        id: MessageId(event.ts.clone()),
        profile: persona,
        thread: ThreadId::new(ID.clone(), instance, external),
        from: UserHandle {
            external: user,
            display,
        },
        text,
        attachments: Vec::new(),
        command,
        surface,
        addressed,
        parent,
        ts: Utc::now(),
        raw: serde_json::json!({
            "channel": event.channel,
            "ts": event.ts,
            "thread_ts": event.thread_ts,
            "channel_type": event.channel_type,
        }),
    })
}

fn parse_text_command(text: &str, call_id: &str, commands: &[CommandSpec]) -> Option<CommandCall> {
    let rest = text.strip_prefix('/')?;
    let (head, args) = split_command(rest);
    let spec = commands
        .iter()
        .find(|command| command.name.as_str().eq_ignore_ascii_case(head))?;
    Some(CommandCall::new(
        call_id.to_string(),
        CommandName::new(spec.name.as_str().to_string()).ok()?,
        args.to_string(),
        serde_json::json!({ "platform": "slack", "command": head }),
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
    match rest.char_indices().find(|(_, ch)| ch.is_whitespace()) {
        Some((index, _)) => (&rest[..index], rest[index..].trim()),
        None => (rest, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_agent_command::CommandSpec;

    const BOT_USER: &str = "UBOT";
    const BOT_ID: &str = "BBOT";

    fn message(text: &str) -> MessageEvent {
        MessageEvent {
            kind: "message".to_string(),
            channel: "C1".to_string(),
            user: Some("U1".to_string()),
            text: Some(text.to_string()),
            ts: "1712345678.000100".to_string(),
            channel_type: Some("channel".to_string()),
            ..MessageEvent::default()
        }
    }

    fn allowlist(values: &[&str]) -> HashSet<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    fn specs() -> Vec<CommandSpec> {
        vec![CommandSpec::raw_string(
            CommandName::new("memory".to_string()).unwrap(),
            "recall",
        )]
    }

    fn convert(event: &MessageEvent) -> IncomingMessage {
        to_incoming(
            event,
            ProfileId::from_slug("main"),
            InstanceId::from_slug("main/slack/slack"),
            BOT_USER,
            &specs(),
            Some("Jane".to_string()),
        )
        .expect("a message")
    }

    #[test]
    fn a_plain_channel_message_is_handled() {
        assert!(should_handle(
            &message("hello"),
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));
    }

    #[test]
    fn our_own_bot_messages_are_skipped() {
        let mut event = message("echo");
        event.bot_id = Some(BOT_ID.to_string());
        assert!(!should_handle(
            &event,
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));
    }

    #[test]
    fn another_bots_message_is_still_handled() {
        let mut event = message("from another app");
        event.bot_id = Some("BOTHER".to_string());
        assert!(should_handle(
            &event,
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));
    }

    #[test]
    fn our_own_user_id_is_skipped_even_without_a_bot_id() {
        let mut event = message("echo");
        event.user = Some(BOT_USER.to_string());
        assert!(!should_handle(
            &event,
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));
    }

    #[test]
    fn subtyped_events_are_skipped() {
        for subtype in ["message_changed", "message_deleted", "channel_join"] {
            let mut event = message("x");
            event.subtype = Some(subtype.to_string());
            assert!(
                !should_handle(&event, BOT_USER, Some(BOT_ID), &HashSet::new()),
                "{subtype} should be skipped"
            );
        }
    }

    #[test]
    fn non_message_events_are_skipped() {
        let mut event = message("x");
        event.kind = "reaction_added".to_string();
        assert!(!should_handle(
            &event,
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));
    }

    #[test]
    fn events_without_a_user_or_coordinates_are_skipped() {
        let mut no_user = message("x");
        no_user.user = None;
        assert!(!should_handle(
            &no_user,
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));

        let mut no_channel = message("x");
        no_channel.channel = String::new();
        assert!(!should_handle(
            &no_channel,
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));

        let mut no_ts = message("x");
        no_ts.ts = String::new();
        assert!(!should_handle(
            &no_ts,
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));
    }

    #[test]
    fn an_empty_allowlist_admits_everyone() {
        assert!(should_handle(
            &message("x"),
            BOT_USER,
            Some(BOT_ID),
            &HashSet::new()
        ));
    }

    #[test]
    fn a_populated_allowlist_gates_by_user() {
        assert!(should_handle(
            &message("x"),
            BOT_USER,
            Some(BOT_ID),
            &allowlist(&["U1"])
        ));
        assert!(!should_handle(
            &message("x"),
            BOT_USER,
            Some(BOT_ID),
            &allowlist(&["U2"])
        ));
    }

    #[test]
    fn a_channel_message_needs_a_mention_to_be_addressed() {
        assert!(!convert(&message("hello")).addressed);
        assert!(convert(&message("<@UBOT> hello")).addressed);
    }

    #[test]
    fn the_bot_mention_is_stripped_from_the_text() {
        assert_eq!(convert(&message("<@UBOT> hello there")).text, "hello there");
    }

    #[test]
    fn other_mentions_survive_as_readable_names() {
        assert_eq!(
            convert(&message("<@UBOT> ping <@U9|dave>")).text,
            "ping @dave"
        );
    }

    #[test]
    fn a_dm_is_always_addressed() {
        let mut event = message("hello");
        event.channel = "D1".to_string();
        event.channel_type = Some("im".to_string());
        let converted = convert(&event);
        assert_eq!(converted.surface, Surface::Dm);
        assert!(converted.addressed);
    }

    #[test]
    fn a_group_dm_still_requires_a_mention() {
        let mut event = message("hello");
        event.channel_type = Some("mpim".to_string());
        let converted = convert(&event);
        assert_eq!(converted.surface, Surface::Channel);
        assert!(!converted.addressed);
    }

    #[test]
    fn a_thread_reply_carries_the_thread_key_and_a_parent() {
        let mut event = message("hello");
        event.thread_ts = Some("1712345600.000100".to_string());
        let converted = convert(&event);
        assert_eq!(converted.surface, Surface::Thread);
        assert_eq!(converted.thread.external, "c:C1:t:1712345600.000100");
        assert_eq!(converted.parent.as_deref(), Some("c:C1"));
    }

    #[test]
    fn a_thread_parent_message_is_not_treated_as_a_reply() {
        let mut event = message("hello");
        event.thread_ts = Some(event.ts.clone());
        let converted = convert(&event);
        assert_eq!(converted.surface, Surface::Channel);
        assert_eq!(converted.thread.external, "c:C1");
        assert!(converted.parent.is_none());
    }

    #[test]
    fn replying_inside_the_bots_thread_counts_as_addressed() {
        let mut event = message("follow up");
        event.thread_ts = Some("1712345600.000100".to_string());
        event.parent_user_id = Some(BOT_USER.to_string());
        assert!(convert(&event).addressed);
    }

    #[test]
    fn a_thread_reply_to_someone_else_is_not_addressed() {
        let mut event = message("follow up");
        event.thread_ts = Some("1712345600.000100".to_string());
        event.parent_user_id = Some("U9".to_string());
        assert!(!convert(&event).addressed);
    }

    #[test]
    fn the_message_id_is_the_slack_timestamp() {
        assert_eq!(convert(&message("x")).id.0, "1712345678.000100");
    }

    #[test]
    fn the_sender_carries_its_resolved_display_name() {
        let converted = convert(&message("x"));
        assert_eq!(converted.from.external, "U1");
        assert_eq!(converted.from.display.as_deref(), Some("Jane"));
    }

    #[test]
    fn a_slash_prefixed_message_becomes_a_command() {
        let converted = convert(&message("/memory what did I say"));
        let call = converted.command.expect("a command call");
        assert_eq!(call.name.as_str(), "memory");
        assert_eq!(call.args, "what did I say");
        assert_eq!(converted.text, "/memory what did I say");
    }

    #[test]
    fn a_bare_command_has_empty_args() {
        let call = convert(&message("/memory"))
            .command
            .expect("a command call");
        assert_eq!(call.args, "");
    }

    #[test]
    fn command_matching_ignores_case() {
        assert!(convert(&message("/MEMORY x")).command.is_some());
    }

    #[test]
    fn an_unknown_slash_word_stays_plain_text() {
        let converted = convert(&message("/unknown thing"));
        assert!(converted.command.is_none());
        assert_eq!(converted.text, "/unknown thing");
    }

    #[test]
    fn a_command_after_a_mention_is_still_recognised() {
        let converted = convert(&message("<@UBOT> /memory x"));
        let call = converted.command.expect("a command call");
        assert_eq!(call.name.as_str(), "memory");
        assert_eq!(call.args, "x");
    }

    #[test]
    fn inbound_escapes_are_decoded_for_the_model() {
        assert_eq!(
            convert(&message("1 &lt; 2 &amp;&amp; 3")).text,
            "1 < 2 && 3"
        );
    }

    #[test]
    fn only_frames_with_an_envelope_get_acked() {
        assert_eq!(
            envelope_of(&Incoming::Event {
                envelope_id: "e1".to_string(),
                event: serde_json::json!({})
            }),
            Some("e1")
        );
        assert_eq!(
            envelope_of(&Incoming::Ignored {
                envelope_id: Some("e2".to_string()),
                kind: "slash_commands".to_string()
            }),
            Some("e2")
        );
        assert_eq!(envelope_of(&Incoming::Hello), None);
        assert_eq!(
            envelope_of(&Incoming::Disconnect {
                reason: "warning".to_string()
            }),
            None
        );
        assert_eq!(envelope_of(&Incoming::Unparsable), None);
    }

    #[test]
    fn split_command_separates_head_from_trimmed_args() {
        assert_eq!(split_command("memory  a b "), ("memory", "a b"));
        assert_eq!(split_command("memory"), ("memory", ""));
    }
}

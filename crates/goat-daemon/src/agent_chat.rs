use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use goat_api::{
    AgentChat, AgentChatItem, AgentChatParams, AgentEntry, AgentList, AgentListOutput,
    AgentMessage, AgentSchedules, AgentSchedulesOutput, AgentSchedulesParams, AgentSend,
    AgentSendOutput, AgentSendParams, Empty, Router, ScheduleEntry, WatchFrom, cursor_for,
};
use goat_channel_desktop::{HubError, Outbound, hub};
use goat_store::{Direction, ScheduleKind, SqliteStore, Store};
use goat_types::{AgentId, ConversationId};
use goat_wire::envelope::{CallError, ErrorCode, Execution};
use tokio::sync::broadcast;

const HISTORY_LIMIT: usize = 200;

fn internal(error: impl std::fmt::Display) -> CallError {
    CallError::new(ErrorCode::Internal, error.to_string()).with_execution(Execution::KnownFailed)
}

fn unknown_agent(slug: &str) -> CallError {
    CallError::new(
        ErrorCode::NotFound,
        format!("agent {slug:?} is not available"),
    )
    .with_execution(Execution::NotStarted)
}

fn agent_id(slug: &str) -> Result<AgentId, CallError> {
    let mut components = Path::new(slug).components();
    let valid = matches!(components.next(), Some(Component::Normal(part)) if part == slug)
        && components.next().is_none()
        && !slug.contains(['\\', '\0']);
    if !valid {
        return Err(
            CallError::new(ErrorCode::Denied, "agent slug must be one directory name")
                .with_execution(Execution::NotStarted),
        );
    }
    Ok(AgentId::from_slug(slug))
}

async fn load_agents(root: PathBuf) -> Result<goat_config::LoadedConfig, CallError> {
    tokio::task::spawn_blocking(move || {
        goat_config::load_from(goat_config::GoatPaths::from_root(root)).map_err(internal)
    })
    .await
    .map_err(internal)?
}

fn send_error(error: &HubError) -> CallError {
    let (code, message) = match error {
        HubError::UnknownAgent => (ErrorCode::NotFound, "agent is not bound to desktop"),
        HubError::Closed => (ErrorCode::HostGone, "agent's desktop inbox is closed"),
        HubError::Full => (ErrorCode::Conflict, "agent's desktop inbox is full"),
    };
    CallError::new(code, message).with_execution(Execution::NotStarted)
}

async fn history(
    store: &SqliteStore,
    agent: AgentId,
    conversation: &ConversationId,
) -> Result<Vec<AgentMessage>, CallError> {
    let count = store
        .message_count(agent, conversation)
        .await
        .map_err(internal)?;
    let offset = count.saturating_sub(HISTORY_LIMIT);
    let rows = store
        .messages_from(agent, conversation, offset, count - offset)
        .await
        .map_err(internal)?;
    let mut messages: Vec<_> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| AgentMessage {
            id: format!("h{}", offset + index),
            outgoing: row.direction == Direction::Out,
            text: row.text,
            ts: row.ts.to_rfc3339(),
            reply_to: row.reply_to.map(|id| id.0),
        })
        .collect();
    let live = hub()
        .messages(agent)
        .ok_or_else(|| send_error(&HubError::UnknownAgent))?;
    for cached in live.iter().rev() {
        let matched = messages.iter().rposition(|stored| {
            stored.outgoing == cached.outgoing
                && if cached.outgoing {
                    cached.reply_to.is_some() && cached.reply_to == stored.reply_to
                } else {
                    cached.ts == stored.ts
                }
        });
        if let Some(index) = matched {
            messages.remove(index);
        }
    }
    messages.extend(live);
    messages.sort_by(|left, right| left.ts.cmp(&right.ts));
    if messages.len() > HISTORY_LIMIT {
        messages.drain(..messages.len() - HISTORY_LIMIT);
    }
    Ok(messages)
}

fn encode(item: &AgentChatItem) -> Result<serde_json::Value, CallError> {
    serde_json::to_value(item).map_err(internal)
}

pub(crate) fn routes(router: Router, root: PathBuf, db_path: PathBuf, epoch: String) -> Router {
    let schedules_root = root.clone();
    let schedules_db = db_path.clone();
    router
        .unary::<AgentList, _, _>(move |_params, _ctx| {
            let root = root.clone();
            async move {
                let loaded = load_agents(root).await?;
                let mut agents: Vec<_> = loaded
                    .agents
                    .into_iter()
                    .map(|agent| {
                        let mut channels: Vec<_> = agent
                            .bindings
                            .into_iter()
                            .map(|binding| binding.name)
                            .collect();
                        let desktop_id = goat_channel_desktop::ID;
                        let desktop = desktop_id.as_str();
                        if !channels.iter().any(|name| name == desktop) {
                            channels.push(desktop.to_owned());
                        }
                        channels.sort_unstable();
                        AgentEntry {
                            slug: agent.slug,
                            display: agent.display,
                            channels,
                            integrations: agent
                                .integrations
                                .into_iter()
                                .map(|integration| integration.name)
                                .collect(),
                        }
                    })
                    .collect();
                agents.sort_unstable_by(|left, right| left.slug.cmp(&right.slug));
                Ok(AgentListOutput { agents })
            }
        })
        .unary::<AgentSend, _, _>(move |params: AgentSendParams, _ctx| async move {
            let agent = agent_id(&params.agent)?;
            let message = hub()
                .send(agent, params.text)
                .map_err(|error| send_error(&error))?;
            Ok(AgentSendOutput { message: message.0 })
        })
        .stream::<AgentChat, _, _>(move |params: AgentChatParams, ctx, sink| {
            let db_path = db_path.clone();
            let epoch = epoch.clone();
            async move {
                let agent = agent_id(&params.agent)?;
                let mut outbound = hub()
                    .subscribe(agent)
                    .ok_or_else(|| unknown_agent(&params.agent))?;
                let conversation = hub()
                    .conversation(agent)
                    .ok_or_else(|| unknown_agent(&params.agent))?;
                let store = SqliteStore::open(&db_path).await.map_err(internal)?;
                let mut seq = match &params.from {
                    WatchFrom::Cursor { cursor } if cursor.epoch == epoch => cursor.seq,
                    _ => 0,
                };
                let mut snapshot_ids = HashSet::new();
                if matches!(params.from, WatchFrom::Snapshot {}) {
                    let messages = history(&store, agent, &conversation).await?;
                    snapshot_ids.extend(messages.iter().map(|message| message.id.clone()));
                    sink.send(encode(&AgentChatItem::Snapshot {
                        cursor: cursor_for(&epoch, seq),
                        messages,
                    })?)
                    .await?;
                }
                loop {
                    let received = tokio::select! {
                        biased;
                        () = ctx.cancel.cancelled() => return Ok(Empty {}),
                        received = outbound.recv() => received,
                    };
                    seq = seq.saturating_add(1);
                    let cursor = cursor_for(&epoch, seq);
                    let item = match received {
                        Ok(Outbound::Created { message }) => {
                            if snapshot_ids.remove(&message.id) {
                                continue;
                            }
                            AgentChatItem::Message { cursor, message }
                        }
                        Ok(Outbound::Updated { id, text }) => {
                            AgentChatItem::Update { cursor, id, text }
                        }
                        Ok(Outbound::Typing { active }) => AgentChatItem::Typing { cursor, active },
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            outbound = outbound.resubscribe();
                            let messages = history(&store, agent, &conversation).await?;
                            snapshot_ids.clear();
                            snapshot_ids.extend(messages.iter().map(|message| message.id.clone()));
                            AgentChatItem::Snapshot { cursor, messages }
                        }
                        Err(broadcast::error::RecvError::Closed) => return Ok(Empty {}),
                    };
                    sink.send(encode(&item)?).await?;
                }
            }
        })
        .unary::<AgentSchedules, _, _>(move |params: AgentSchedulesParams, _ctx| {
            let root = schedules_root.clone();
            let db_path = schedules_db.clone();
            async move {
                let agent = agent_id(&params.agent)?;
                if !load_agents(root)
                    .await?
                    .agents
                    .iter()
                    .any(|configured| configured.id == agent)
                {
                    return Err(unknown_agent(&params.agent));
                }
                let store = SqliteStore::open(&db_path).await.map_err(internal)?;
                let schedules = store
                    .list_active_schedules(agent)
                    .await
                    .map_err(internal)?
                    .into_iter()
                    .map(|(schedule, next_run)| {
                        let (kind, when) = match schedule.schedule {
                            ScheduleKind::Once(at) => ("once", at.to_rfc3339()),
                            ScheduleKind::Cron(expression) => ("cron", expression),
                        };
                        ScheduleEntry {
                            id: schedule.id,
                            instruction: schedule.instruction,
                            kind: kind.to_owned(),
                            when,
                            next_run: next_run.map(|at| at.to_rfc3339()),
                        }
                    })
                    .collect();
                Ok(AgentSchedulesOutput { schedules })
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_api::Grant;
    use goat_channel::{BindOutput, Channel, ChannelBinding, ChannelSecrets};
    use goat_channel_desktop::DesktopChannel;
    use goat_store::NewSchedule;
    use goat_types::{InstanceId, OutgoingBody};
    use goat_wire::WireConn;
    use goat_wire::envelope::{Frame, Role};
    use goat_wire::peer::{Peer, RejectAll, StreamHandle, StreamMsg, spawn};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    type Conn = WireConn<tokio::io::DuplexStream, Frame, Frame>;

    fn connect(root: &Path) -> (Peer, Peer) {
        let router = routes(
            Router::new([Grant::Any]),
            root.to_path_buf(),
            root.join("goat.db"),
            "chat-test".to_owned(),
        );
        let (client, daemon) = tokio::io::duplex(1024 * 1024);
        let (client_sink, client_source) = Conn::new(client).split();
        let (daemon_sink, daemon_source) = Conn::new(daemon).split();
        let closed = CancellationToken::new();
        let daemon = spawn(
            Role::Daemon,
            Box::pin(daemon_sink),
            Box::pin(daemon_source),
            Arc::new(router),
            closed.clone(),
        );
        let client = spawn(
            Role::Client,
            Box::pin(client_sink),
            Box::pin(client_source),
            Arc::new(RejectAll),
            closed,
        );
        (client, daemon)
    }

    fn slug(root: &Path) -> String {
        format!("agent-{}", root.file_name().unwrap().to_string_lossy())
    }

    fn configure(root: &Path, slug: &str, config: &Value) {
        let dir = root.join("agents").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.md"), "You help with desktop work.").unwrap();
        std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    }

    async fn bind(slug: &str) -> BindOutput {
        Arc::new(DesktopChannel)
            .bind(
                AgentId::from_slug(slug),
                ChannelBinding {
                    instance: InstanceId::from_slug(&format!("{slug}/desktop/desktop")),
                    config: json!({}),
                    commands: Vec::new(),
                    secrets: ChannelSecrets::new(),
                },
            )
            .await
            .unwrap()
    }

    async fn chat(client: &Peer, slug: &str) -> StreamHandle {
        client
            .handle
            .open_stream(
                "agent.chat",
                1,
                json!({"agent": slug, "from": {"type": "Snapshot"}}),
            )
            .await
            .unwrap()
    }

    async fn next(stream: &mut StreamHandle) -> AgentChatItem {
        let received = tokio::time::timeout(Duration::from_secs(5), stream.recv())
            .await
            .unwrap();
        let Some(StreamMsg::Item { item, .. }) = received else {
            panic!("expected chat item, got {received:?}");
        };
        serde_json::from_value(item).unwrap()
    }

    #[tokio::test]
    async fn missing_unbound_and_unsafe_agents_are_refused_before_sending() {
        let root = tempfile::tempdir().unwrap();
        let configured = slug(root.path());
        configure(
            root.path(),
            &configured,
            &json!({"model": "anthropic/claude-x"}),
        );
        let (client, _daemon) = connect(root.path());
        let missing = format!("{configured}-missing");
        for name in [&missing, &configured] {
            let error = client
                .handle
                .call("agent.send", 1, json!({"agent": name, "text": "hello"}))
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::NotFound);
            assert_eq!(error.execution, Some(Execution::NotStarted));
            let mut stream = chat(&client, name).await;
            let Some(StreamMsg::End(Err(error))) = stream.recv().await else {
                panic!("an unbound agent must refuse the chat stream");
            };
            assert_eq!(error.code, ErrorCode::NotFound);
        }
        let error = client
            .handle
            .call("agent.schedules", 1, json!({"agent": missing}))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        for name in ["", "..", "../outside", "/outside", "a\\b", "a\0b"] {
            for method in ["agent.send", "agent.schedules"] {
                let error = client
                    .handle
                    .call(method, 1, json!({"agent": name, "text": "hello"}))
                    .await
                    .unwrap_err();
                assert_eq!(error.code, ErrorCode::Denied);
                assert_eq!(error.execution, Some(Execution::NotStarted));
            }
            let mut stream = chat(&client, name).await;
            let Some(StreamMsg::End(Err(error))) = stream.recv().await else {
                panic!("an unsafe slug must refuse the chat stream");
            };
            assert_eq!(error.code, ErrorCode::Denied);
        }
    }

    #[tokio::test]
    async fn list_and_schedules_use_loaded_config_and_only_active_agent_schedules() {
        let root = tempfile::tempdir().unwrap();
        let slug = slug(root.path());
        configure(
            root.path(),
            &slug,
            &json!({
                "model": "anthropic/claude-x",
                "display": "Assistant",
                "channels": {"discord": {}},
                "integrations": {"github": {}}
            }),
        );
        configure(root.path(), "invalid", &json!({"display": "No model"}));
        let (client, _daemon) = connect(root.path());
        let listed: AgentListOutput = serde_json::from_value(
            client
                .handle
                .call("agent.list", 1, Value::Null)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            listed.agents,
            vec![AgentEntry {
                slug: slug.clone(),
                display: "Assistant".to_owned(),
                channels: vec!["desktop".to_owned(), "discord".to_owned()],
                integrations: vec!["github".to_owned()],
            }]
        );

        let agent = AgentId::from_slug(&slug);
        let store = SqliteStore::open(&root.path().join("goat.db"))
            .await
            .unwrap();
        store.ensure_agent(agent, &slug, "Assistant").await.unwrap();
        let conversation = ConversationId::new(
            goat_channel_desktop::ID,
            InstanceId::from_slug(&slug),
            "main",
        );
        store
            .ensure_conversation(&conversation, agent)
            .await
            .unwrap();
        let at = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let once = NewSchedule {
            agent,
            instruction: "Review the screen".to_owned(),
            tools: Vec::new(),
            origin_conv: conversation,
            schedule: ScheduleKind::Once(at),
            timezone: None,
            created_by_msg_id: None,
        };
        let once_id = store.insert_schedule(once.clone()).await.unwrap();
        store
            .insert_schedule_run(once_id, at, once.instruction.clone())
            .await
            .unwrap();
        let cron_id = store
            .insert_schedule(NewSchedule {
                instruction: "Daily review".to_owned(),
                schedule: ScheduleKind::Cron("0 9 * * *".to_owned()),
                ..once.clone()
            })
            .await
            .unwrap();
        let cancelled_id = store.insert_schedule(once.clone()).await.unwrap();
        store.cancel_schedule(cancelled_id).await.unwrap();
        let other = AgentId::from_slug(&format!("{slug}-other"));
        store.ensure_agent(other, "other", "Other").await.unwrap();
        store
            .insert_schedule(NewSchedule {
                agent: other,
                ..once
            })
            .await
            .unwrap();
        let mut output: AgentSchedulesOutput = serde_json::from_value(
            client
                .handle
                .call("agent.schedules", 1, json!({"agent": slug}))
                .await
                .unwrap(),
        )
        .unwrap();
        output
            .schedules
            .sort_unstable_by_key(|schedule| schedule.id);
        assert_eq!(
            output.schedules,
            vec![
                ScheduleEntry {
                    id: once_id,
                    instruction: "Review the screen".to_owned(),
                    kind: "once".to_owned(),
                    when: at.to_rfc3339(),
                    next_run: Some(at.to_rfc3339()),
                },
                ScheduleEntry {
                    id: cron_id,
                    instruction: "Daily review".to_owned(),
                    kind: "cron".to_owned(),
                    when: "0 9 * * *".to_owned(),
                    next_run: None,
                },
            ]
        );
    }

    #[tokio::test]
    async fn chat_broadcasts_accepted_messages_edits_and_typing_to_every_client() {
        let root = tempfile::tempdir().unwrap();
        let slug = slug(root.path());
        let (handle, mut incoming) = bind(&slug).await;
        let (first, _first_daemon) = connect(root.path());
        let (second, _second_daemon) = connect(root.path());
        let mut one = chat(&first, &slug).await;
        let mut two = chat(&second, &slug).await;
        for stream in [&mut one, &mut two] {
            let AgentChatItem::Snapshot { messages, .. } = next(stream).await else {
                panic!("expected initial snapshot");
            };
            assert!(messages.is_empty());
        }
        let sent: AgentSendOutput = serde_json::from_value(
            first
                .handle
                .call("agent.send", 1, json!({"agent": slug, "text": "hello"}))
                .await
                .unwrap(),
        )
        .unwrap();
        let received = incoming.recv().await.unwrap();
        assert_eq!(received.id.0, sent.message);
        assert_eq!(received.text, "hello");
        for stream in [&mut one, &mut two] {
            let AgentChatItem::Message { cursor, message } = next(stream).await else {
                panic!("expected accepted user message");
            };
            assert_eq!(cursor, cursor_for("chat-test", 1));
            assert_eq!(message.id, sent.message);
            assert_eq!(message.text, "hello");
            assert!(!message.outgoing);
        }
        let reply = handle
            .send(
                &received.conversation,
                OutgoingBody::Text("Working".to_owned()),
                Some(received.id),
            )
            .await
            .unwrap();
        for stream in [&mut one, &mut two] {
            let AgentChatItem::Message { cursor, message } = next(stream).await else {
                panic!("expected agent reply");
            };
            assert_eq!(cursor, cursor_for("chat-test", 2));
            assert_eq!(message.id, reply.message_id.0);
            assert_eq!(message.text, "Working");
            assert_eq!(message.reply_to.as_deref(), Some(sent.message.as_str()));
            assert!(message.outgoing);
        }
        handle
            .edit(&reply, OutgoingBody::Text("Finished".to_owned()))
            .await
            .unwrap();
        for stream in [&mut one, &mut two] {
            assert_eq!(
                next(stream).await,
                AgentChatItem::Update {
                    cursor: cursor_for("chat-test", 3),
                    id: reply.message_id.0.clone(),
                    text: "Finished".to_owned(),
                }
            );
        }
        let typing = handle.typing(&received.conversation).await.unwrap();
        for stream in [&mut one, &mut two] {
            assert_eq!(
                next(stream).await,
                AgentChatItem::Typing {
                    cursor: cursor_for("chat-test", 4),
                    active: true,
                }
            );
        }
        drop(typing);
        for stream in [&mut one, &mut two] {
            assert_eq!(
                next(stream).await,
                AgentChatItem::Typing {
                    cursor: cursor_for("chat-test", 5),
                    active: false,
                }
            );
        }
    }

    #[tokio::test]
    async fn reopened_chat_keeps_history_ids_when_the_recent_window_moves() {
        let root = tempfile::tempdir().unwrap();
        let slug = slug(root.path());
        let (_handle, _incoming) = bind(&slug).await;
        let agent = AgentId::from_slug(&slug);
        let conversation = hub().conversation(agent).unwrap();
        let store = SqliteStore::open(&root.path().join("goat.db"))
            .await
            .unwrap();
        store.ensure_agent(agent, &slug, &slug).await.unwrap();
        store
            .append_incoming_text(agent, &conversation, "Before the window")
            .await
            .unwrap();
        for index in 1..=200 {
            store
                .append_outgoing_text(agent, &conversation, &format!("Reply {index}"), None)
                .await
                .unwrap();
        }
        let (client, _daemon) = connect(root.path());
        let mut stream = chat(&client, &slug).await;
        let AgentChatItem::Snapshot {
            messages: before, ..
        } = next(&mut stream).await
        else {
            panic!("expected restored history");
        };
        assert_eq!(before.len(), HISTORY_LIMIT);
        assert_eq!(before[0].id, "h1");
        assert_eq!(before[0].text, "Reply 1");
        assert!(before[0].outgoing);
        assert_eq!(before[199].id, "h200");
        assert_eq!(before[199].text, "Reply 200");
        drop(stream);
        store
            .append_incoming_text(agent, &conversation, "One more question")
            .await
            .unwrap();
        let mut stream = chat(&client, &slug).await;
        let AgentChatItem::Snapshot {
            messages: after, ..
        } = next(&mut stream).await
        else {
            panic!("expected updated history");
        };
        assert_eq!(after.len(), HISTORY_LIMIT);
        assert_eq!(&before[1..], &after[..199]);
        assert_eq!(after[199].id, "h201");
        assert_eq!(after[199].text, "One more question");
        assert!(!after[199].outgoing);
    }

    #[tokio::test]
    async fn reopening_an_active_reply_restores_its_live_id_without_duplicate_history() {
        let root = tempfile::tempdir().unwrap();
        let slug = slug(root.path());
        let (handle, mut incoming) = bind(&slug).await;
        let agent = AgentId::from_slug(&slug);
        let conversation = hub().conversation(agent).unwrap();
        let store = SqliteStore::open(&root.path().join("goat.db"))
            .await
            .unwrap();
        store.ensure_agent(agent, &slug, &slug).await.unwrap();
        store
            .append_outgoing_text(agent, &conversation, "An older scheduled message", None)
            .await
            .unwrap();
        hub().send(agent, "A question".to_owned()).unwrap();
        let received = incoming.recv().await.unwrap();
        store.append_incoming(&received).await.unwrap();
        let reply = handle
            .send(
                &conversation,
                OutgoingBody::Text("Partial".to_owned()),
                Some(received.id.clone()),
            )
            .await
            .unwrap();
        store
            .append_outgoing_text(agent, &conversation, "Partial", Some(&received.id))
            .await
            .unwrap();
        handle
            .edit(&reply, OutgoingBody::Text("More complete".to_owned()))
            .await
            .unwrap();
        let (client, _daemon) = connect(root.path());
        let mut stream = chat(&client, &slug).await;
        let AgentChatItem::Snapshot { messages, .. } = next(&mut stream).await else {
            panic!("expected restored active conversation");
        };
        assert_eq!(
            messages
                .iter()
                .map(|message| (message.id.as_str(), message.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("h0", "An older scheduled message"),
                (received.id.0.as_str(), "A question"),
                (reply.message_id.0.as_str(), "More complete"),
            ]
        );
        handle
            .edit(&reply, OutgoingBody::Text("Complete".to_owned()))
            .await
            .unwrap();
        let AgentChatItem::Update { id, text, .. } = next(&mut stream).await else {
            panic!("expected the active reply to continue streaming");
        };
        assert_eq!(id, messages[2].id);
        assert_eq!(text, "Complete");
    }

    #[tokio::test]
    async fn a_lagged_chat_resets_to_the_latest_cached_reply_then_keeps_streaming() {
        let root = tempfile::tempdir().unwrap();
        let slug = slug(root.path());
        let (handle, _incoming) = bind(&slug).await;
        let conversation = hub().conversation(AgentId::from_slug(&slug)).unwrap();
        let reply = handle
            .send(
                &conversation,
                OutgoingBody::Text("Starting".to_owned()),
                None,
            )
            .await
            .unwrap();
        let (client, _daemon) = connect(root.path());
        let mut stream = chat(&client, &slug).await;
        let AgentChatItem::Snapshot { messages, .. } = next(&mut stream).await else {
            panic!("expected initial snapshot");
        };
        assert_eq!(messages[0].id, reply.message_id.0);
        for index in 0..512 {
            handle
                .edit(&reply, OutgoingBody::Text(format!("Revision {index}")))
                .await
                .unwrap();
        }
        let AgentChatItem::Snapshot { messages, cursor } = next(&mut stream).await else {
            panic!("lag must reset the conversation snapshot");
        };
        assert_eq!(messages[0].id, reply.message_id.0);
        assert_eq!(messages[0].text, "Revision 511");
        handle
            .edit(&reply, OutgoingBody::Text("Final".to_owned()))
            .await
            .unwrap();
        assert_eq!(
            next(&mut stream).await,
            AgentChatItem::Update {
                cursor: cursor.next(),
                id: reply.message_id.0,
                text: "Final".to_owned(),
            }
        );
    }
}

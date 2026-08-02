use goat_protocol::{
    Effort, Event, ModelTarget, NotifyKind, RewindDraft, RewindPoint, RewindScope, SkillInfo,
    ThreadSummary, ToolCall, ToolCallId, ToolOutcome, TranscriptEntry,
};
use goat_provider::{ContentBlock, Message, MessageRole};
use goat_store::CodeStore as Store;
use goat_tools::ToolRegistry;
use tokio::sync::mpsc;

use crate::{
    Ctx,
    prompt::build_system_prompt,
    tools_exec::{call_display, summarize_line},
};

pub(crate) fn parse_content_blocks(body: &str) -> Vec<ContentBlock> {
    serde_json::from_str::<Vec<ContentBlock>>(body).unwrap_or_else(|_| {
        vec![ContentBlock::Text {
            text: body.to_owned(),
        }]
    })
}

pub(crate) async fn resolve_thread_cwd(
    ctx: &Ctx<'_>,
    stored_thread: Option<i64>,
) -> std::path::PathBuf {
    match stored_thread {
        Some(tid) => ctx
            .store
            .get_thread(tid)
            .await
            .ok()
            .flatten()
            .map(|thread| thread.cwd)
            .filter(|cwd| !cwd.is_empty())
            .map_or_else(|| ctx.cwd.to_path_buf(), std::path::PathBuf::from),
        None => ctx.cwd.to_path_buf(),
    }
}

pub(crate) async fn handle_list_threads(
    store: &Store,
    cwd: &std::path::Path,
    events: &mpsc::Sender<Event>,
) {
    let cwd = cwd.display().to_string();
    let threads = match store.list_threads_in(cwd, 50).await {
        Ok(threads) => threads,
        Err(err) => {
            tracing::warn!(error = %err, "failed to list threads for picker");
            Vec::new()
        }
    };
    let summaries = threads
        .into_iter()
        .map(|thread| ThreadSummary {
            model: format!("{}/{}", thread.provider, thread.model),
            title: thread
                .title
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("{}/{}", thread.provider, thread.model)),
            id: thread.id,
            updated_at: thread.updated_at,
            live: false,
        })
        .collect();
    let _ = events
        .send(Event::ThreadsListed { threads: summaries })
        .await;
}

pub(crate) async fn handle_rename(
    store: &Store,
    thread_id: Option<i64>,
    title: String,
    events: &mpsc::Sender<Event>,
) {
    let Some(tid) = thread_id else {
        let _ = events
            .send(Event::Notify {
                kind: NotifyKind::Error,
                message: "no active conversation to rename".to_owned(),
            })
            .await;
        return;
    };
    match store.update_thread_title(tid, title.clone()).await {
        Ok(()) => {
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Success,
                    message: format!("renamed to \"{title}\""),
                })
                .await;
        }
        Err(err) => {
            tracing::warn!(%err, "failed to rename thread");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Error,
                    message: "failed to rename conversation".to_owned(),
                })
                .await;
        }
    }
}

pub(crate) async fn handle_list_rewind_points(
    checkpoints: &crate::checkpoint::CheckpointTracker,
    thread_id: Option<i64>,
    events: &mpsc::Sender<Event>,
) {
    let Some(thread_id) = thread_id else {
        let _ = events
            .send(Event::RewindPointsListed { points: Vec::new() })
            .await;
        return;
    };
    let stored = match checkpoints.points(thread_id).await {
        Ok(points) => points,
        Err(err) => {
            tracing::warn!(%err, "failed to list rewind checkpoints");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Error,
                    message: "could not load rewind checkpoints".to_owned(),
                })
                .await;
            return;
        }
    };
    let mut later_code_changes = false;
    let points = stored
        .into_iter()
        .map(|checkpoint| {
            later_code_changes |= checkpoint.touched;
            RewindPoint {
                checkpoint_id: checkpoint.id,
                prompt: checkpoint.draft,
                created_at: checkpoint.created_at,
                code_changes: checkpoint.files_available && later_code_changes,
            }
        })
        .collect();
    let _ = events.send(Event::RewindPointsListed { points }).await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_rewind(
    store: &Store,
    checkpoints: &crate::checkpoint::CheckpointTracker,
    skills: &[SkillInfo],
    tools: &ToolRegistry,
    instructions: Option<&str>,
    date: &str,
    checkpoint_id: i64,
    scope: RewindScope,
    state: &mut crate::SessionState,
    events: &mpsc::Sender<Event>,
) {
    let Some(thread_id) = state.thread_id else {
        rewind_error(events, "no active conversation to rewind").await;
        return;
    };
    let points = match checkpoints.points(thread_id).await {
        Ok(points) => points,
        Err(err) => {
            tracing::warn!(%err, "failed to read rewind checkpoint");
            rewind_error(events, "could not load that rewind checkpoint").await;
            return;
        }
    };
    let Some(checkpoint) = points
        .into_iter()
        .find(|checkpoint| checkpoint.id == checkpoint_id)
    else {
        rewind_error(
            events,
            "that checkpoint is no longer part of this conversation",
        )
        .await;
        return;
    };
    let attachments = serde_json::from_str(&checkpoint.attachments).unwrap_or_default();
    let restore_code = matches!(scope, RewindScope::Code | RewindScope::CodeAndConversation);
    let restore_conversation = matches!(
        scope,
        RewindScope::Conversation | RewindScope::CodeAndConversation
    );
    let mut report = None;
    if restore_code {
        let Some(thread) = store.get_thread(thread_id).await.ok().flatten() else {
            rewind_error(events, "could not resolve the checkpoint workspace").await;
            return;
        };
        match checkpoints
            .restore(thread_id, checkpoint_id, std::path::Path::new(&thread.cwd))
            .await
        {
            Ok(restored) => report = Some(restored),
            Err(err) => {
                tracing::warn!(%err, "failed to restore checkpoint files");
                rewind_error(events, &format!("could not restore code: {err}")).await;
                return;
            }
        }
    }
    if restore_conversation {
        if let Err(err) = store
            .set_thread_head(
                thread_id,
                checkpoint.parent_message_id,
                crate::persist::now_ms(),
            )
            .await
        {
            tracing::warn!(%err, "failed to rewind conversation head");
            rewind_error(events, "could not restore the conversation").await;
            return;
        }
        checkpoints.clear();
        handle_resume(
            store,
            skills,
            tools,
            instructions,
            date,
            thread_id,
            state,
            events,
        )
        .await;
        let _ = events
            .send(Event::ConversationRewound {
                draft: RewindDraft {
                    text: checkpoint.draft,
                    attachments,
                },
            })
            .await;
    }
    if let Some(report) = report {
        let restored = match report.restored {
            1 => "restored 1 file".to_owned(),
            count => format!("restored {count} files"),
        };
        let message = if report.skipped == 0 {
            restored
        } else {
            format!(
                "{restored}; skipped {} linked or unsupported paths",
                report.skipped
            )
        };
        let _ = events
            .send(Event::Notify {
                kind: if report.skipped == 0 {
                    NotifyKind::Success
                } else {
                    NotifyKind::Info
                },
                message,
            })
            .await;
    }
}

async fn rewind_error(events: &mpsc::Sender<Event>, message: &str) {
    let _ = events
        .send(Event::Notify {
            kind: NotifyKind::Error,
            message: message.to_owned(),
        })
        .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_resume(
    store: &Store,
    skills: &[SkillInfo],
    tools: &ToolRegistry,
    instructions: Option<&str>,
    date: &str,
    tid: i64,
    state: &mut crate::SessionState,
    events: &mpsc::Sender<Event>,
) {
    let thread = match store.get_thread(tid).await {
        Ok(Some(thread)) => thread,
        Ok(None) => {
            tracing::warn!(thread_id = tid, "resume requested for unknown thread");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Error,
                    message: format!("conversation {tid} was not found"),
                })
                .await;
            return;
        }
        Err(err) => {
            tracing::warn!(thread_id = tid, error = %err, "failed to read thread for resume");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Error,
                    message: "could not load that conversation".to_owned(),
                })
                .await;
            return;
        }
    };
    let new_target = ModelTarget {
        provider: thread.provider.clone(),
        model: thread.model.clone(),
        account: thread.account.clone(),
        effort: thread.effort.as_deref().and_then(Effort::parse),
    };
    let messages = match store.get_messages(tid).await {
        Ok(messages) => messages,
        Err(err) => {
            tracing::warn!(thread_id = tid, error = %err, "failed to read messages for resume");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Error,
                    message: "could not load that conversation's messages".to_owned(),
                })
                .await;
            return;
        }
    };
    let compactions = match store.compactions_for_thread(tid).await {
        Ok(compactions) => compactions,
        Err(err) => {
            tracing::warn!(thread_id = tid, error = %err, "failed to read compactions for resume");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Error,
                    message: "could not load that conversation's history".to_owned(),
                })
                .await;
            return;
        }
    };
    let active_message_ids: std::collections::HashSet<i64> =
        messages.iter().map(|message| message.id).collect();
    let compactions: Vec<_> = compactions
        .into_iter()
        .filter(|compaction| {
            compaction.after_message_id == 0
                || active_message_ids.contains(&compaction.after_message_id)
        })
        .collect();
    let mut parsed: Vec<(i64, MessageRole, Vec<ContentBlock>)> = Vec::new();
    let mut entries: Vec<TranscriptEntry> = Vec::new();
    let mut tool_uses: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut tool_seq: u64 = 0;
    let mut next_compaction = 0usize;
    for stored in messages {
        while next_compaction < compactions.len()
            && compactions[next_compaction].after_message_id < stored.id
        {
            let compaction = &compactions[next_compaction];
            entries.push(TranscriptEntry::Compaction {
                tokens_before: u32::try_from(compaction.tokens_before).unwrap_or(0),
                tokens_after: u32::try_from(compaction.tokens_after).unwrap_or(0),
            });
            next_compaction += 1;
        }
        if stored.role == "shell" {
            let content = parse_content_blocks(&stored.body);
            if let Some(ContentBlock::Text { text }) = content.first() {
                match crate::shell::decode(text) {
                    Some((command, output)) => {
                        entries.push(TranscriptEntry::Shell { command, output });
                    }
                    None => entries.push(TranscriptEntry::User {
                        text: text.clone(),
                        attachments: Vec::new(),
                    }),
                }
            }
            parsed.push((stored.id, MessageRole::User, content));
            continue;
        }
        let role = match stored.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => continue,
        };
        let content = parse_content_blocks(&stored.body);
        for block in &content {
            match block {
                ContentBlock::Text { text } => match role {
                    MessageRole::User => entries.push(TranscriptEntry::User {
                        text: text.clone(),
                        attachments: content
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Image { media_type, data } => {
                                    Some(goat_protocol::InputAttachment {
                                        media_type: media_type.clone(),
                                        data: data.clone(),
                                        label: "image".to_owned(),
                                    })
                                }
                                _ => None,
                            })
                            .collect(),
                    }),
                    MessageRole::Assistant => {
                        entries.push(TranscriptEntry::Assistant { text: text.clone() });
                    }
                    MessageRole::System => {}
                },
                ContentBlock::ToolUse { id, name, input } => {
                    tool_uses.insert(id.clone(), (name.clone(), input.to_string()));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    if let Some((name, input)) = tool_uses.remove(tool_use_id) {
                        tool_seq += 1;
                        let display = call_display(tools, &name, &input);
                        let summary = if *is_error {
                            summarize_line(&ContentBlock::tool_result_text(content))
                        } else {
                            None
                        };
                        entries.push(TranscriptEntry::Tool {
                            call: ToolCall {
                                id: ToolCallId(tool_seq),
                                name,
                                display,
                            },
                            outcome: ToolOutcome {
                                ok: !is_error,
                                summary,
                                image: None,
                            },
                        });
                    }
                }
                ContentBlock::Thinking { text, .. } => {
                    if matches!(role, MessageRole::Assistant) {
                        entries.push(TranscriptEntry::Thinking { text: text.clone() });
                    }
                }
                ContentBlock::Image { .. } | ContentBlock::RedactedThinking { .. } => {}
            }
        }
        parsed.push((stored.id, role, content));
    }
    while next_compaction < compactions.len() {
        let compaction = &compactions[next_compaction];
        entries.push(TranscriptEntry::Compaction {
            tokens_before: u32::try_from(compaction.tokens_before).unwrap_or(0),
            tokens_after: u32::try_from(compaction.tokens_after).unwrap_or(0),
        });
        next_compaction += 1;
    }
    let mut new_history: Vec<(Message, Option<i64>)> = vec![(
        Message::text(
            MessageRole::System,
            build_system_prompt(
                std::path::Path::new(&thread.cwd),
                skills,
                instructions,
                date,
            ),
        ),
        None,
    )];
    if let Some(latest) = compactions.last() {
        new_history.push((crate::compaction::summary_message(&latest.summary), None));
        for (id, role, content) in parsed {
            let include = latest.preserved_message_ids.contains(&id)
                || latest
                    .tail_from_message_id
                    .is_some_and(|tail| id >= tail && id <= latest.after_message_id)
                || id > latest.after_message_id;
            if include {
                new_history.push((Message { role, content }, Some(id)));
            }
        }
    } else {
        for (id, role, content) in parsed {
            new_history.push((Message { role, content }, Some(id)));
        }
    }
    state.conversation.replace(new_history);
    state.tracker.invalidate();
    let context_tokens = Some(state.tracker.estimate(state.conversation.messages(), &[]));
    state.thread_id = Some(tid);
    state.target = Some(new_target.clone());
    let _ = events.send(Event::ThreadBound { thread_id: tid }).await;
    let _ = events
        .send(Event::ConversationRestored {
            target: new_target,
            entries,
            context_tokens,
            compaction_threshold: None,
        })
        .await;
    if store.last_turn_interrupted(tid).await.unwrap_or(false) {
        let _ = events
            .send(Event::Notify {
                kind: NotifyKind::Info,
                message: "⚠ the previous turn was interrupted (daemon restarted)".to_owned(),
            })
            .await;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_resume_latest(
    store: &Store,
    skills: &[SkillInfo],
    tools: &ToolRegistry,
    instructions: Option<&str>,
    date: &str,
    cwd: &std::path::Path,
    state: &mut crate::SessionState,
    events: &mpsc::Sender<Event>,
) {
    let cwd_key = cwd.display().to_string();
    match store.latest_thread_in(cwd_key).await {
        Ok(Some(thread)) => {
            handle_resume(
                store,
                skills,
                tools,
                instructions,
                date,
                thread.id,
                state,
                events,
            )
            .await;
        }
        Ok(None) => {
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Info,
                    message: "no previous conversation in this directory".to_owned(),
                })
                .await;
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to look up latest thread for resume");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Info,
                    message: "could not load a previous conversation".to_owned(),
                })
                .await;
        }
    }
}

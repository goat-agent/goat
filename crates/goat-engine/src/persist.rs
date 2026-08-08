use std::time::{SystemTime, UNIX_EPOCH};

use goat_code_store::{
    CodeStore as Store, CreatedMessage, NewMessage, NewThread, NewToolCall, NewTurn,
};
use goat_protocol::{Effort, Event, ModelTarget, TaskId, ToolOutcome};
use goat_provider::{ContentBlock, Message, MessageRole};

use crate::{Ctx, TurnIds, turn::TurnEnd};

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| {
            tracing::warn!("system clock before unix epoch");
            0
        },
        |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
    )
}

pub(crate) fn effort_string(effort: Option<Effort>) -> Option<String> {
    effort.map(|e| e.as_str().to_owned())
}

pub(crate) async fn create_tool_call_record(
    ctx: &Ctx,
    ids: &TurnIds,
    vendor_id: &str,
    name: &str,
    input_json: &str,
) -> Option<i64> {
    let (Some(tid), Some(turn)) = (ids.stored_thread, ids.turn_db_id) else {
        return None;
    };
    match ctx
        .store
        .create_tool_call(NewToolCall {
            thread_id: tid,
            turn_id: turn,
            call_id: vendor_id.to_owned(),
            name: name.to_owned(),
            input: input_json.to_owned(),
            status: "running".to_owned(),
            started_at: now_ms(),
        })
        .await
    {
        Ok(id) => Some(id),
        Err(err) => {
            tracing::warn!(%err, "failed to create tool call record");
            None
        }
    }
}

pub(crate) async fn finish_tool_db(ctx: &Ctx, db_id: Option<i64>, outcome: &ToolOutcome) {
    let Some(db) = db_id else {
        return;
    };
    let status = if outcome.ok { "done" } else { "error" }.to_owned();
    if let Err(err) = ctx
        .store
        .finish_tool_call(db, status, outcome.summary.clone(), now_ms())
        .await
    {
        tracing::warn!(%err, "failed to finish tool call");
    }
}

pub(crate) fn thread_title(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let title: String = trimmed.chars().take(60).collect();
    Some(title)
}

pub(crate) async fn ensure_thread(
    store: &Store,
    cwd: &std::path::Path,
    thread_id: &mut Option<i64>,
    target: &ModelTarget,
    title: Option<String>,
) -> Option<i64> {
    if let Some(tid) = thread_id {
        return Some(*tid);
    }
    let timestamp = now_ms();
    let cwd = cwd.display().to_string();
    match store
        .create_thread(NewThread {
            cwd,
            title,
            provider: target.provider.clone(),
            model: target.model.clone(),
            account: target.account.clone(),
            effort: effort_string(target.effort),
            created_at: timestamp,
            updated_at: timestamp,
        })
        .await
    {
        Ok(id) => {
            *thread_id = Some(id);
            Some(id)
        }
        Err(err) => {
            tracing::warn!(%err, "failed to create thread");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn init_db_turn(
    ctx: &Ctx,
    id: TaskId,
    message: &Message,
    text: &str,
    draft: &str,
    attachments: &[goat_protocol::InputAttachment],
    target: &ModelTarget,
    thread_id: &mut Option<i64>,
    checkpoint: bool,
) -> TurnIds {
    let title = if text.trim().is_empty() {
        attachments
            .first()
            .map(|attachment| format!("Image: {}", attachment.label))
    } else {
        thread_title(text)
    };
    let stored_thread = ensure_thread(&ctx.store, &ctx.cwd, thread_id, target, title).await;
    if let Some(tid) = stored_thread {
        let _ = ctx.events.send(Event::ThreadBound { thread_id: tid }).await;
    }
    let (turn_db_id, user_message_db_id) = if let Some(tid) = stored_thread {
        let body = serde_json::to_string(&message.content).unwrap_or_else(|_| text.to_owned());
        let created_message = match ctx
            .store
            .create_message(NewMessage {
                thread_id: tid,
                turn_id: None,
                role: "user".to_owned(),
                body,
                created_at: now_ms(),
            })
            .await
        {
            Ok(row) => Some(row),
            Err(err) => {
                tracing::warn!(%err, "failed to persist user message");
                None
            }
        };
        if checkpoint
            && let Some(created) = &created_message
            && let Err(err) = ctx
                .checkpoints
                .begin(tid, created, draft.to_owned(), attachments, &ctx.cwd)
                .await
        {
            tracing::warn!(%err, "failed to create code checkpoint");
            let _ = ctx
                .events
                .send(Event::Notify {
                    kind: goat_protocol::NotifyKind::Error,
                    message: "could not create a rewind checkpoint".to_owned(),
                })
                .await;
        } else if !checkpoint {
            ctx.checkpoints.clear();
        }
        let turn_db_id = ctx
            .store
            .create_turn(NewTurn {
                thread_id: tid,
                task_id: i64::try_from(id.0).unwrap_or(i64::MAX),
                provider: target.provider.clone(),
                model: target.model.clone(),
                account: target.account.clone(),
                effort: effort_string(target.effort),
                status: "running".to_owned(),
                started_at: now_ms(),
            })
            .await
            .ok();
        (turn_db_id, created_message.map(|message| message.id))
    } else {
        (None, None)
    };
    TurnIds {
        stored_thread,
        turn_db_id,
        user_message_db_id,
    }
}

pub(crate) async fn persist_message(
    ctx: &Ctx,
    ids: &TurnIds,
    message: &Message,
) -> Option<CreatedMessage> {
    let role = match message.role {
        MessageRole::System => return None,
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };
    let tid = ids.stored_thread?;
    let Ok(body) = serde_json::to_string(&message.content) else {
        return None;
    };
    match ctx
        .store
        .create_message(NewMessage {
            thread_id: tid,
            turn_id: ids.turn_db_id,
            role: role.to_owned(),
            body,
            created_at: now_ms(),
        })
        .await
    {
        Ok(row) => Some(row),
        Err(err) => {
            tracing::warn!(%err, "failed to persist message");
            None
        }
    }
}

pub(crate) async fn persist_shell_message(ctx: &Ctx, thread_id: i64, encoded: &str) -> Option<i64> {
    let body = serde_json::to_string(&vec![ContentBlock::Text {
        text: encoded.to_owned(),
    }])
    .unwrap_or_else(|_| encoded.to_owned());
    match ctx
        .store
        .create_message(NewMessage {
            thread_id,
            turn_id: None,
            role: "shell".to_owned(),
            body,
            created_at: now_ms(),
        })
        .await
    {
        Ok(row) => Some(row.id),
        Err(err) => {
            tracing::warn!(%err, "failed to persist shell message");
            None
        }
    }
}

pub(crate) async fn finalize_turn(ctx: &Ctx, id: TaskId, outcome: &TurnEnd, ids: &TurnIds) {
    match outcome {
        TurnEnd::Done => {
            if let Some(turn) = ids.turn_db_id
                && let Err(err) = ctx
                    .store
                    .finish_turn(turn, "done".to_owned(), now_ms())
                    .await
            {
                tracing::warn!(%err, "failed to finish turn");
            }
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: false,
                })
                .await;
        }
        TurnEnd::Interrupted => {
            if let Some(turn) = ids.turn_db_id
                && let Err(err) = ctx
                    .store
                    .finish_turn(turn, "interrupted".to_owned(), now_ms())
                    .await
            {
                tracing::warn!(%err, "failed to finish turn");
            }
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: true,
                })
                .await;
        }
        TurnEnd::Failed(message, hint) => {
            let _ = ctx
                .events
                .send(Event::Error {
                    id: Some(id),
                    message: message.clone(),
                    hint: hint.clone(),
                })
                .await;
            if let Some(turn) = ids.turn_db_id
                && let Err(err) = ctx
                    .store
                    .finish_turn(turn, "error".to_owned(), now_ms())
                    .await
            {
                tracing::warn!(%err, "failed to finish turn");
            }
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: true,
                })
                .await;
        }
        TurnEnd::Shutdown => {}
    }
}

use std::fmt::Write as _;

use goat_protocol::{Event, InputAttachment, Op, TaskId};
use goat_provider::{ContentBlock, Message, MessageRole, Provider, ToolDefinition};
use goat_tool::{SandboxPolicy, ToolRegistry, ToolSandbox};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Flow, Run, SessionContext, SessionState,
    accounts::provider_for,
    conversations::resolve_conversation_cwd,
    persist::{
        conversation_title, effort_string, ensure_conversation, finalize_turn, init_db_turn,
        now_ms, persist_shell_message,
    },
    prompt::build_system_prompt,
    rounds::{LoopOutcome, core_loop},
    shell,
    tools_exec::{ToolAvailability, build_tool_defs},
};

#[derive(Clone, Copy)]
enum AskAvailability {
    Available,
    Unavailable,
}

impl AskAvailability {
    const fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }
}

pub(crate) fn user_message(text: &str, attachments: &[InputAttachment]) -> Message {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(ContentBlock::Text {
            text: text.to_owned(),
        });
    }
    for attachment in attachments {
        content.push(ContentBlock::Image {
            media_type: attachment.media_type.clone(),
            data: attachment.data.clone(),
        });
    }
    Message {
        role: MessageRole::User,
        content,
    }
}

fn top_regime(
    ctx: &SessionContext,
    provider: &dyn Provider,
    availability: ToolAvailability,
) -> Vec<ToolDefinition> {
    build_tool_defs(ctx, provider, None, availability)
}

const SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);
pub(crate) const SHELL_INTERRUPTED: &str = "[interrupted]";

enum ShellEnd {
    Done(String),
    Interrupted,
    Shutdown,
}

async fn run_shell_command(tools: &ToolRegistry, command: &str, cwd: &std::path::Path) -> String {
    let tool_ctx = match ToolSandbox::new(cwd) {
        Ok(tool_ctx) => tool_ctx,
        Err(err) => return err.to_string(),
    };
    let Some(tool) = tools.get(goat_tool_shell::SHELL_TOOL) else {
        return "shell tool unavailable".to_owned();
    };
    let input = serde_json::json!({
        "command": command,
        "timeout_ms": SHELL_TIMEOUT.as_millis()
    })
    .to_string();
    match tool.run(&input, &tool_ctx).await {
        Ok(output) => output.as_text().unwrap_or_default().to_owned(),
        Err(err) if err.class() == goat_tool::ToolErrorClass::Timeout => {
            format!("[timed out after {}m]", SHELL_TIMEOUT.as_secs() / 60)
        }
        Err(err) => err.to_string(),
    }
}

pub(crate) enum TurnEnd {
    Done,
    Interrupted,
    Failed(String, Option<String>),
    Shutdown,
}

pub(crate) async fn emit_task_error(
    ctx: &SessionContext,
    id: TaskId,
    message: String,
    hint: Option<String>,
) {
    let _ = ctx
        .events
        .send(Event::Error {
            id: Some(id),
            message,
            hint,
        })
        .await;
    let _ = ctx
        .events
        .send(Event::TaskDone {
            id,
            interrupted: true,
        })
        .await;
}

pub(crate) async fn handle_idle_op(op: Op, ctx: &SessionContext, state: &mut SessionState) {
    let store = &ctx.store;
    let events = &ctx.events;
    let conversation_id = state.conversation_id;
    match op {
        Op::ProcessKill { process } => {
            let _ = ctx.background.kill(process, None).await;
        }
        Op::ProcessWatch { process, on } => {
            let _ = ctx.background.set_watch(process, on).await;
        }
        Op::SelectModel { target: chosen } => {
            if let Some(tid) = conversation_id
                && let Err(err) = store
                    .update_conversation_model(
                        tid,
                        chosen.provider.clone(),
                        chosen.model.clone(),
                        chosen.account.clone(),
                        effort_string(chosen.effort),
                        now_ms(),
                    )
                    .await
            {
                tracing::warn!(%err, "failed to update conversation model");
            }
            state.target = Some(chosen.clone());
            let _ = events.send(Event::ModelSelected { target: chosen }).await;
        }
        Op::SetMode { mode } => {
            apply_mode(ctx, state, mode).await;
        }
        Op::RenameConversation { title } => {
            crate::conversations::handle_rename(store, conversation_id, title, events).await;
        }
        Op::ListConversations {} => {
            crate::conversations::handle_list_conversations(store, &ctx.cwd, events).await;
        }
        Op::Login { .. }
        | Op::AddAccount { .. }
        | Op::RemoveAccount { .. }
        | Op::ListRewindPoints { .. }
        | Op::Rewind { .. }
        | Op::Resume { .. }
        | Op::ResumeLatest { .. } => {
            let _ = events
                .send(Event::Notify {
                    kind: goat_protocol::NotifyKind::Info,
                    message: "ignored while a task is running — try again after it finishes"
                        .to_owned(),
                })
                .await;
        }
        _ => {}
    }
}

async fn bind_plan_path(
    ctx: &SessionContext,
    state: &mut SessionState,
    conversation_id: Option<i64>,
    seed: &str,
) {
    if !state.mode.is_plan() || state.plan_path.is_some() {
        return;
    }
    let Some(tid) = conversation_id else { return };
    let Some(dir) = goat_config::plans_dir() else {
        return;
    };
    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(%err, "failed to create plans directory");
        return;
    }
    let seed = seed.to_owned();
    let Ok(path) =
        tokio::task::spawn_blocking(move || goat_tool_plan::resolve_path(&dir, tid, &seed)).await
    else {
        return;
    };
    state.plan_path = Some(path.clone());
    let _ = ctx
        .events
        .send(Event::ModeChanged {
            mode: state.mode,
            plan_path: Some(path.display().to_string()),
        })
        .await;
}

pub(crate) async fn apply_mode(
    ctx: &SessionContext,
    state: &mut SessionState,
    mode: goat_protocol::Mode,
) {
    state.mode = mode;
    if !mode.is_plan() {
        state.plan_path = None;
    }
    let _ = ctx
        .events
        .send(Event::ModeChanged {
            mode,
            plan_path: state.plan_path.as_ref().map(|p| p.display().to_string()),
        })
        .await;
}

pub(crate) async fn handle_plan_decision(
    ctx: &SessionContext,
    decision: goat_protocol::PlanDecision,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    let Some(input) = plan_decision_input(ctx, state, &decision) else {
        return Flow::Continue;
    };
    if matches!(decision, goat_protocol::PlanDecision::Approve {}) {
        apply_mode(ctx, state, goat_protocol::Mode::Normal).await;
    }
    run_turn_chain(
        ctx,
        input,
        std::collections::VecDeque::new(),
        state,
        ops,
        AskAvailability::Available,
    )
    .await
}

fn plan_decision_input(
    ctx: &SessionContext,
    state: &SessionState,
    decision: &goat_protocol::PlanDecision,
) -> Option<crate::UserInput> {
    let (text, display) = match decision {
        goat_protocol::PlanDecision::Approve {} => {
            let path = state.plan_path.as_ref()?;
            (
                goat_tool_plan::approved_input(path),
                "(plan approved)".to_owned(),
            )
        }
        goat_protocol::PlanDecision::Reject { feedback } => (
            goat_tool_plan::rejected_input(feedback),
            "(plan rejected)".to_owned(),
        ),
    };
    Some(crate::UserInput {
        id: TaskId(
            ctx.plan_ids
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ),
        text,
        display: Some(display),
        attachments: Vec::new(),
        checkpoint: false,
    })
}

enum TurnFlow {
    Idle,
    Done(std::collections::VecDeque<crate::UserInput>),
    Shutdown,
}

enum PumpAction {
    Continue,
    Interrupt,
    Shutdown,
}

async fn pump_op(
    ctx: &SessionContext,
    id: TaskId,
    op: Option<Op>,
    steering: &crate::SteeringQueue,
    deferred: &mut Vec<Op>,
) -> PumpAction {
    match op {
        Some(Op::SubmitMessage {
            id: msg_id,
            text: msg_text,
            display,
            attachments,
        }) => {
            steering
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back(crate::UserInput {
                    id: msg_id,
                    text: msg_text,
                    display,
                    attachments,
                    checkpoint: true,
                });
            PumpAction::Continue
        }
        Some(Op::DequeueMessage { id: msg_id }) => {
            let removed = {
                let mut queue = steering
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                queue
                    .iter()
                    .rposition(|queued| queued.id == msg_id)
                    .and_then(|index| queue.remove(index))
            };
            if let Some(queued) = removed {
                let _ = ctx
                    .events
                    .send(Event::MessageDequeued {
                        id: queued.id,
                        text: queued.text,
                        display: queued.display,
                        attachments: queued.attachments,
                    })
                    .await;
            }
            PumpAction::Continue
        }
        Some(Op::Interrupt { id: target_id }) if target_id == id => PumpAction::Interrupt,
        Some(Op::Shutdown {}) | None => PumpAction::Shutdown,
        Some(op) => {
            deferred.push(op);
            PumpAction::Continue
        }
    }
}

fn wake_notice(updates: &[(goat_protocol::RunId, crate::background::RunUpdate)]) -> String {
    let mut body = String::from(
        "<environment-notice>\nAutomated runtime signal — this is NOT a message from the user. Do not reply to it conversationally, do not acknowledge or thank it, and do not repeat an earlier waiting reply. Background work finished or produced output you had not read; act only if it now needs action (read it, fix it, or move on), otherwise produce no user-facing text and continue what you were doing.\n",
    );
    for (id, update) in updates {
        let status = match (update.state, update.ok) {
            (goat_protocol::ProcessState::Running, _) => "running".to_owned(),
            (goat_protocol::ProcessState::Exited, Some(true)) => "done".to_owned(),
            (goat_protocol::ProcessState::Exited, Some(false)) => "failed".to_owned(),
            (goat_protocol::ProcessState::Exited, None) => match update.exit_code {
                Some(code) => format!("exited(code {code})"),
                None => "exited".to_owned(),
            },
        };
        let _ = write!(
            body,
            "\n[{} #{id} · {} · {status}]\n",
            update.label, update.title
        );
        if update.output.trim().is_empty() {
            body.push_str("(no output)\n");
        } else {
            body.push_str(update.output.trim_end());
            body.push('\n');
        }
    }
    body.push_str("</environment-notice>");
    body
}

pub(crate) async fn handle_wake(
    ctx: &SessionContext,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    let updates = ctx.background.take_pending_updates().await;
    if updates.is_empty() {
        return Flow::Continue;
    }
    let body = wake_notice(&updates);

    let wake_id = TaskId(
        ctx.wake_ids
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    );
    run_turn_chain(
        ctx,
        crate::UserInput {
            id: wake_id,
            text: body,
            display: Some("(background activity)".to_owned()),
            attachments: Vec::new(),
            checkpoint: false,
        },
        std::collections::VecDeque::new(),
        state,
        ops,
        AskAvailability::Unavailable,
    )
    .await
}

pub(crate) async fn handle_turn(
    ctx: &SessionContext,
    id: TaskId,
    text: String,
    display: Option<String>,
    attachments: Vec<InputAttachment>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    run_turn_chain(
        ctx,
        crate::UserInput {
            id,
            text,
            display,
            attachments,
            checkpoint: true,
        },
        std::collections::VecDeque::new(),
        state,
        ops,
        AskAvailability::Available,
    )
    .await
}

async fn run_turn_chain(
    ctx: &SessionContext,
    input: crate::UserInput,
    seed: std::collections::VecDeque<crate::UserInput>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
    ask_availability: AskAvailability,
) -> Flow {
    let mut next = Some((input, seed));
    let mut pending: Vec<Op> = Vec::new();
    while let Some((turn_input, turn_seed)) = next.take() {
        let (flow, deferred) =
            run_one_turn(ctx, turn_input, turn_seed, state, ops, ask_availability).await;
        pending.extend(deferred);
        match flow {
            TurnFlow::Shutdown => return Flow::Shutdown,
            TurnFlow::Idle => {}
            TurnFlow::Done(mut leftover) => {
                if let Some(next_input) = leftover.pop_front() {
                    next = Some((next_input, leftover));
                }
            }
        }
    }
    drain_deferred(ctx, pending, state, ops).await
}

async fn drain_deferred(
    ctx: &SessionContext,
    deferred: Vec<Op>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    for op in deferred {
        match op {
            Op::Compact { id, instructions } => {
                if let Flow::Shutdown =
                    Box::pin(handle_compact(ctx, id, instructions, state, ops)).await
                {
                    return Flow::Shutdown;
                }
            }
            Op::SubmitShell { id, command } => {
                if let Flow::Shutdown = Box::pin(handle_shell(ctx, id, &command, state, ops)).await
                {
                    return Flow::Shutdown;
                }
            }
            Op::ResolvePlan { decision, .. } => {
                if let Flow::Shutdown =
                    Box::pin(handle_plan_decision(ctx, decision, state, ops)).await
                {
                    return Flow::Shutdown;
                }
            }
            other => {
                handle_idle_op(other, ctx, state).await;
            }
        }
    }
    Flow::Continue
}

pub(crate) async fn handle_shell(
    ctx: &SessionContext,
    id: TaskId,
    command: &str,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }
    let stored_conversation = match state.target.as_ref() {
        Some(resolved) => {
            ensure_conversation(
                &ctx.store,
                &ctx.cwd,
                &mut state.conversation_id,
                resolved,
                conversation_title(&format!("! {command}")),
            )
            .await
        }
        None => None,
    };
    let cwd = resolve_conversation_cwd(ctx, stored_conversation).await;
    let steering: crate::SteeringQueue = std::sync::Mutex::new(std::collections::VecDeque::new());
    let mut deferred: Vec<Op> = Vec::new();
    let outcome = {
        let work = run_shell_command(&ctx.tools, command, &cwd);
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                output = &mut work => break ShellEnd::Done(output),
                maybe_op = ops.recv() => match pump_op(ctx, id, maybe_op, &steering, &mut deferred).await {
                    PumpAction::Continue => {}
                    PumpAction::Interrupt => break ShellEnd::Interrupted,
                    PumpAction::Shutdown => break ShellEnd::Shutdown,
                },
            }
        }
    };

    let output = match outcome {
        ShellEnd::Shutdown => return Flow::Shutdown,
        ShellEnd::Interrupted => SHELL_INTERRUPTED.to_owned(),
        ShellEnd::Done(output) => output,
    };

    let encoded = shell::encode(command, &output);
    if state.conversation.is_empty() {
        state.conversation.push(
            Message::text(
                MessageRole::System,
                build_system_prompt(
                    &ctx.cwd,
                    &ctx.skills,
                    ctx.instructions.as_deref(),
                    &ctx.date,
                    state.plan_prompt_path(),
                ),
            ),
            None,
        );
    }
    let db_id = match stored_conversation {
        Some(tid) => persist_shell_message(ctx, tid, &encoded).await,
        None => None,
    };
    state
        .conversation
        .push(Message::text(MessageRole::User, encoded), db_id);

    let _ = ctx.events.send(Event::ShellDone { id, output }).await;
    let _ = ctx
        .events
        .send(Event::TaskDone {
            id,
            interrupted: false,
        })
        .await;

    if let Flow::Shutdown = drain_deferred(ctx, deferred, state, ops).await {
        return Flow::Shutdown;
    }
    let mut captured = std::mem::take(
        &mut *steering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    drop(steering);
    if let Some(next_input) = captured.pop_front() {
        return Box::pin(run_turn_chain(
            ctx,
            next_input,
            captured,
            state,
            ops,
            AskAvailability::Available,
        ))
        .await;
    }
    Flow::Continue
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_compact(
    ctx: &SessionContext,
    id: TaskId,
    instructions: Option<String>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    if state.conversation.is_empty() {
        let _ = ctx
            .events
            .send(Event::Notify {
                kind: goat_protocol::NotifyKind::Info,
                message: "nothing to compact".to_owned(),
            })
            .await;
        return Flow::Continue;
    }
    let Some(resolved) = state.target.clone() else {
        let _ = ctx
            .events
            .send(Event::Notify {
                kind: goat_protocol::NotifyKind::Error,
                message: "no model selected · /config to connect a provider".to_owned(),
            })
            .await;
        return Flow::Continue;
    };
    let resolved_provider = provider_for(
        ctx,
        &resolved.account,
        &goat_provider::ProviderId::from(resolved.provider.as_str()),
    );
    let Some(provider) = resolved_provider else {
        let _ = ctx
            .events
            .send(Event::Notify {
                kind: goat_protocol::NotifyKind::Error,
                message: format!("unknown provider: {}", resolved.provider),
            })
            .await;
        return Flow::Continue;
    };
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }
    let cwd = resolve_conversation_cwd(ctx, state.conversation_id).await;
    let tool_defs = top_regime(
        ctx,
        provider.as_ref(),
        ToolAvailability {
            delegation: true,
            asking: true,
            planning: false,
        },
    );
    let ids = crate::TurnIds {
        stored_conversation: state.conversation_id,
        turn_db_id: None,
        user_message_db_id: None,
    };
    let steering: crate::SteeringQueue = std::sync::Mutex::new(std::collections::VecDeque::new());
    let run = Run::top(id, &ids, &steering);
    let env = crate::LoopEnv {
        provider,
        target: resolved,
        tool_defs,
        cwd,
        allow_delegate: true,
        interactive: true,
        plan: false,
        plan_path: None,
        exec_policy: SandboxPolicy::Full,
    };
    let token = CancellationToken::new();
    let mut shutdown = false;
    let mut deferred: Vec<Op> = Vec::new();

    let result = {
        let work = crate::compaction::compact(
            ctx,
            &run,
            &env,
            &mut state.conversation,
            &mut state.tracker,
            instructions.as_deref(),
            &token,
        );
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                outcome = &mut work => break outcome,
                maybe_op = ops.recv() => match pump_op(ctx, id, maybe_op, &steering, &mut deferred).await {
                    PumpAction::Continue => {}
                    PumpAction::Interrupt => token.cancel(),
                    PumpAction::Shutdown => {
                        shutdown = true;
                        token.cancel();
                    }
                },
            }
        }
    };

    match result {
        Ok(_) => {
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: false,
                })
                .await;
        }
        Err(crate::compaction::CompactionError::Cancelled) => {
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: true,
                })
                .await;
        }
        Err(crate::compaction::CompactionError::Failed(message)) => {
            emit_task_error(
                ctx,
                id,
                format!("compaction failed: {message}"),
                Some("/clear to reset the conversation".to_owned()),
            )
            .await;
        }
    }
    if shutdown {
        return Flow::Shutdown;
    }
    if let Flow::Shutdown = drain_deferred(ctx, deferred, state, ops).await {
        return Flow::Shutdown;
    }
    let mut captured = std::mem::take(
        &mut *steering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    drop(steering);
    if let Some(next_input) = captured.pop_front() {
        return Box::pin(run_turn_chain(
            ctx,
            next_input,
            captured,
            state,
            ops,
            AskAvailability::Available,
        ))
        .await;
    }
    Flow::Continue
}

#[allow(clippy::too_many_lines)]
async fn run_one_turn(
    ctx: &SessionContext,
    input: crate::UserInput,
    seed: std::collections::VecDeque<crate::UserInput>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
    ask_availability: AskAvailability,
) -> (TurnFlow, Vec<Op>) {
    let id = input.id;
    let text = input.text;
    let draft = input.display.as_deref().unwrap_or(&text).to_owned();
    let attachments = input.attachments;
    let checkpoint = input.checkpoint;
    let Some(resolved) = state.target.clone() else {
        emit_task_error(
            ctx,
            id,
            "no model selected".to_owned(),
            Some("/config to connect a provider".to_owned()),
        )
        .await;
        return (TurnFlow::Idle, Vec::new());
    };
    let resolved_provider = provider_for(
        ctx,
        &resolved.account,
        &goat_provider::ProviderId::from(resolved.provider.as_str()),
    );
    let Some(provider) = resolved_provider else {
        emit_task_error(
            ctx,
            id,
            format!("unknown provider: {}", resolved.provider),
            Some("/config to select a provider".to_owned()),
        )
        .await;
        return (TurnFlow::Idle, Vec::new());
    };

    let message = user_message(&text, &attachments);
    let ids = init_db_turn(
        ctx,
        id,
        &message,
        &text,
        &draft,
        &attachments,
        &resolved,
        &mut state.conversation_id,
        checkpoint,
    )
    .await;
    bind_plan_path(ctx, state, ids.stored_conversation, &text).await;
    let system = build_system_prompt(
        &ctx.cwd,
        &ctx.skills,
        ctx.instructions.as_deref(),
        &ctx.date,
        state.plan_prompt_path(),
    );
    if state.conversation.is_empty() {
        state
            .conversation
            .push(Message::text(MessageRole::System, system), None);
    } else if state.conversation.set_system(system) {
        state.tracker.invalidate();
    }
    state.conversation.push(message, ids.user_message_db_id);
    if ctx
        .events
        .send(Event::UserMessage {
            id,
            text: text.clone(),
            display: input.display.clone(),
            attachments: attachments.clone(),
        })
        .await
        .is_err()
    {
        finalize_turn(ctx, id, &TurnEnd::Shutdown, &ids).await;
        return (TurnFlow::Shutdown, Vec::new());
    }
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        finalize_turn(ctx, id, &TurnEnd::Shutdown, &ids).await;
        return (TurnFlow::Shutdown, Vec::new());
    }

    let cwd = resolve_conversation_cwd(ctx, ids.stored_conversation).await;
    let tool_defs = top_regime(
        ctx,
        provider.as_ref(),
        ToolAvailability {
            delegation: true,
            asking: ask_availability.is_available(),
            planning: state.mode.is_plan(),
        },
    );
    let steering: crate::SteeringQueue = std::sync::Mutex::new(seed);
    let run = Run::top(id, &ids, &steering);
    let env = crate::LoopEnv {
        provider,
        target: resolved,
        tool_defs,
        cwd,
        allow_delegate: true,
        interactive: ask_availability.is_available(),
        plan: state.mode.is_plan(),
        plan_path: state.plan_path.clone(),
        exec_policy: SandboxPolicy::Full,
    };
    let token = CancellationToken::new();
    let mut shutdown = false;
    let mut deferred: Vec<Op> = Vec::new();

    let outcome = {
        let core = core_loop(
            ctx,
            &run,
            &env,
            &token,
            &mut state.conversation,
            &mut state.tracker,
        );
        tokio::pin!(core);
        loop {
            tokio::select! {
                biased;
                result = &mut core => break result,
                maybe_op = ops.recv() => {
                    if let Some(Op::Answer { call, answers, .. }) = &maybe_op {
                        if let Some(tx) = ctx.asks.lock().await.remove(call) {
                            let _ = tx.send(answers.clone());
                            let _ = ctx
                                .events
                                .send(Event::AskDismissed { id, call: *call })
                                .await;
                        }
                        continue;
                    }
                    match pump_op(ctx, id, maybe_op, &steering, &mut deferred).await {
                        PumpAction::Continue => {}
                        PumpAction::Interrupt => token.cancel(),
                        PumpAction::Shutdown => {
                            shutdown = true;
                            token.cancel();
                        }
                    }
                }
            }
        }
    };

    let turn_end = match outcome {
        LoopOutcome::Completed => TurnEnd::Done,
        LoopOutcome::Failed(message, hint) => TurnEnd::Failed(message, hint),
        LoopOutcome::Cancelled => {
            if shutdown {
                TurnEnd::Shutdown
            } else {
                TurnEnd::Interrupted
            }
        }
    };
    finalize_turn(ctx, id, &turn_end, &ids).await;
    if matches!(turn_end, TurnEnd::Shutdown) {
        return (TurnFlow::Shutdown, deferred);
    }

    if matches!(turn_end, TurnEnd::Done) {
        let leftover = std::mem::take(
            &mut *steering
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if !leftover.is_empty() {
            return (TurnFlow::Done(leftover), deferred);
        }
    }
    (TurnFlow::Idle, deferred)
}

#[cfg(test)]
mod tests {
    use super::wake_notice;
    use crate::background::RunUpdate;
    use goat_protocol::{ProcessState, RunId};

    fn bash(title: &str, output: &str, code: i32) -> (RunId, RunUpdate) {
        (
            RunId(3),
            RunUpdate {
                label: "bash".to_owned(),
                title: title.to_owned(),
                output: output.to_owned(),
                state: ProcessState::Exited,
                exit_code: Some(code),
                ok: None,
            },
        )
    }

    fn subagent(title: &str, report: &str, ok: bool) -> (RunId, RunUpdate) {
        (
            RunId(7),
            RunUpdate {
                label: "subagent".to_owned(),
                title: title.to_owned(),
                output: report.to_owned(),
                state: ProcessState::Exited,
                exit_code: None,
                ok: Some(ok),
            },
        )
    }

    #[test]
    fn a_wake_names_each_kind_and_carries_its_result() {
        let notice = wake_notice(&[
            bash("cargo build", "error[E0432]", 1),
            subagent("explore — map auth", "auth goes through goat-auth", true),
        ]);
        assert!(notice.contains("[bash #3 · cargo build · exited(code 1)]"));
        assert!(notice.contains("error[E0432]"));
        assert!(notice.contains("[subagent #7 · explore — map auth · done]"));
        assert!(notice.contains("auth goes through goat-auth"));
    }

    #[test]
    fn a_failed_subagent_is_marked_failed_not_exited() {
        let notice = wake_notice(&[subagent("general", "context overflow", false)]);
        assert!(notice.contains("· failed]"), "got: {notice}");
        assert!(!notice.contains("exited"), "got: {notice}");
    }

    #[test]
    fn a_wake_is_never_addressed_as_a_user_message() {
        let notice = wake_notice(&[bash("true", "", 0)]);
        assert!(notice.contains("NOT a message from the user"));
        assert!(notice.contains("(no output)"));
    }
}

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use goat_protocol::{
    AccountEntry, Event, Mode, ModelEntry, ModelTarget, Op, ProcessInfo, ProcessState,
    RateLimitSnapshot, RunId, SkillInfo, TaskId, ToolCall, ToolCallId, TranscriptEntry, Usage,
};
use goat_wire::{
    ClientId, ModeEntry, RateLimitEntry, RetryEntry, ServerFrame, SessionId, SessionLiveState,
    UsageEntry,
};
use tokio::sync::Mutex;
use tokio::sync::mpsc;

pub(crate) struct Subscriber {
    pub(crate) client: ClientId,
    pub(crate) sender: mpsc::Sender<ServerFrame>,
    pub(crate) lagged: tokio_util::sync::CancellationToken,
}

pub(crate) struct SessionInner {
    pub(crate) id: SessionId,
    pub(crate) cwd: String,
    pub(crate) created_at: i64,
    pub(crate) ops: mpsc::Sender<Op>,
    pub(crate) log: VecDeque<(u64, Event)>,
    pub(crate) next_seq: u64,
    pub(crate) next_task: u64,
    pub(crate) subscribers: Vec<Subscriber>,
    pub(crate) state: SessionLiveState,
    pub(crate) transcript: LiveTranscript,
    pub(crate) restore_target: Option<ModelTarget>,
    pub(crate) context_tokens: Option<u32>,
    pub(crate) compaction_threshold: Option<u32>,
    pub(crate) tokens: u64,
    pub(crate) open_asks: usize,
    pub(crate) live_processes: usize,
    pub(crate) conversation_id: Option<i64>,
    pub(crate) awaits_restore: bool,
    pub(crate) ready: Arc<tokio::sync::Notify>,
    pub(crate) resurrected: std::collections::HashSet<u64>,
    pub(crate) pending_attaches: usize,
    pub(crate) skills: Vec<SkillInfo>,
    pub(crate) accounts: Vec<AccountEntry>,
    pub(crate) model_list: Vec<ModelEntry>,
    pub(crate) selected_target: Option<ModelTarget>,
    pub(crate) rate_limits: HashMap<(String, String), (RateLimitSnapshot, i64)>,
    pub(crate) usage: HashMap<(String, String), UsageState>,
    pub(crate) mode: Mode,
    pub(crate) plan_path: Option<String>,
    pub(crate) processes: HashMap<RunId, ProcessInfo>,
    pub(crate) active: Option<TaskId>,
    pub(crate) retry: Option<RetryState>,
    pub(crate) asks: HashMap<ToolCallId, Event>,
    pub(crate) subagents: HashMap<TaskId, Event>,
    pub(crate) plan: Option<Event>,
    pub(crate) state_ready: bool,
}

#[derive(Clone, Default)]
pub(crate) struct LiveTranscript {
    entries: Vec<TranscriptEntry>,
    text: HashMap<TaskId, String>,
    thinking: HashMap<TaskId, String>,
    tools: HashMap<ToolCallId, (TaskId, ToolCall)>,
    shells: HashMap<TaskId, String>,
}

#[derive(Clone)]
pub(crate) struct UsageState {
    usage: Usage,
    context_window: Option<u32>,
    compaction_threshold: Option<u32>,
}

pub(crate) struct RetryState {
    id: TaskId,
    attempt: u32,
    max_attempts: u32,
    until: tokio::time::Instant,
    reason: String,
    resets_at: Option<i64>,
}

#[derive(Clone)]
pub(crate) struct LiveSession {
    pub(crate) inner: Arc<Mutex<SessionInner>>,
}

pub(crate) struct PersistEvent {
    pub(crate) conversation_id: i64,
    pub(crate) prompt: Option<PromptAction>,
}

pub(crate) enum PromptAction {
    Open {
        call_id: String,
        kind: String,
        payload: String,
        task_id: u64,
    },
    Close {
        call_id: String,
    },
}

impl LiveTranscript {
    fn restore(&mut self, entries: &[TranscriptEntry]) {
        self.entries = entries.to_vec();
        self.text.clear();
        self.thinking.clear();
        self.tools.clear();
        self.shells.clear();
    }

    fn flush_thinking(&mut self, id: TaskId) {
        if let Some(text) = self.thinking.remove(&id) {
            self.entries.push(TranscriptEntry::Thinking { text });
        }
    }

    fn append_text(&mut self, id: TaskId, chunk: &str) {
        self.flush_thinking(id);
        self.text.entry(id).or_default().push_str(chunk);
    }

    fn finish_text(&mut self, id: TaskId, value: &str) {
        self.flush_thinking(id);
        self.text.remove(&id);
        self.entries.push(TranscriptEntry::Assistant {
            text: value.to_owned(),
        });
    }

    fn append_thinking(&mut self, id: TaskId, chunk: &str) {
        self.thinking.entry(id).or_default().push_str(chunk);
    }

    fn pending_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        let mut thinking: Vec<_> = self.thinking.iter().collect();
        thinking.sort_by_key(|(id, _)| **id);
        for (id, chunk) in thinking {
            events.push(Event::ThinkingDelta {
                id: *id,
                chunk: chunk.clone(),
            });
        }
        let mut text: Vec<_> = self.text.iter().collect();
        text.sort_by_key(|(id, _)| **id);
        for (id, chunk) in text {
            events.push(Event::TextDelta {
                id: *id,
                chunk: chunk.clone(),
            });
        }
        let mut tools: Vec<_> = self.tools.values().collect();
        tools.sort_by_key(|(_, call)| call.id);
        for (id, call) in tools {
            events.push(Event::ToolStarted {
                id: *id,
                call: call.clone(),
            });
        }
        events
    }

    fn apply(&mut self, event: &Event) {
        match event {
            Event::ConversationRestored { entries, .. } => self.restore(entries),
            Event::UserMessage {
                text,
                display,
                system,
                attachments,
                ..
            } => self.entries.push(TranscriptEntry::User {
                text: display.clone().unwrap_or_else(|| text.clone()),
                display: display.clone(),
                system: *system,
                attachments: attachments.clone(),
            }),
            Event::TextDelta { id, chunk } => self.append_text(*id, chunk),
            Event::TextDone { id, text } => self.finish_text(*id, text),
            Event::ThinkingDelta { id, chunk } => self.append_thinking(*id, chunk),
            Event::Retrying { id, .. } => {
                self.text.remove(id);
            }
            Event::ToolStarted { id, call } => {
                self.tools.insert(call.id, (*id, call.clone()));
            }
            Event::ToolDone { call, outcome, .. } => {
                if let Some((_, call)) = self.tools.remove(call) {
                    self.entries.push(TranscriptEntry::Tool {
                        call,
                        outcome: outcome.clone(),
                    });
                }
            }
            Event::CompactionDone {
                ok: true,
                tokens_before,
                tokens_after,
                ..
            } => self.entries.push(TranscriptEntry::Compaction {
                tokens_before: *tokens_before,
                tokens_after: *tokens_after,
            }),
            Event::ShellDone { id, output } => {
                if let Some(command) = self.shells.remove(id) {
                    self.entries.push(TranscriptEntry::Shell {
                        command,
                        output: output.clone(),
                    });
                }
            }
            Event::TaskDone { id, interrupted } => {
                self.flush_thinking(*id);
                if let Some(text) = self.text.remove(id) {
                    self.entries.push(TranscriptEntry::Assistant {
                        text: if *interrupted {
                            format!("{text}\n\n(interrupted)")
                        } else {
                            text
                        },
                    });
                }
                let unfinished: Vec<_> = self
                    .tools
                    .extract_if(|_, (task, _)| task == id)
                    .map(|(_, (_, call))| call)
                    .collect();
                if *interrupted {
                    for call in unfinished {
                        self.entries.push(TranscriptEntry::Tool {
                            call,
                            outcome: goat_protocol::ToolOutcome {
                                ok: false,
                                summary: None,
                                body: None,
                                image: None,
                                git: None,
                            },
                        });
                    }
                }
                self.shells.remove(id);
            }
            _ => {}
        }
    }

    fn record_op(&mut self, id: TaskId, op: &Op) {
        if let Op::SubmitShell { command, .. } = op {
            self.shells.insert(id, command.clone());
        }
    }
}

impl SessionInner {
    pub(crate) fn allocate_task(&mut self) -> goat_protocol::TaskId {
        let id = self.next_task;
        self.next_task += 1;
        goat_protocol::TaskId(id)
    }

    fn cache_state_event(&mut self, event: &Event) {
        match event {
            Event::SkillsChanged { skills } => {
                self.skills.clone_from(skills);
                self.state_ready = true;
                self.ready.notify_waiters();
            }
            Event::AccountsChanged { providers } => self.accounts.clone_from(providers),
            Event::ModelListChanged { entries } => self.model_list.clone_from(entries),
            Event::ModelSelected { target } => self.selected_target = Some(target.clone()),
            Event::RateLimits {
                provider,
                account,
                snapshot,
                cached_at,
            } => {
                self.rate_limits.insert(
                    (provider.clone(), account.clone()),
                    (snapshot.clone(), *cached_at),
                );
            }
            Event::ModeChanged { mode, plan_path } => {
                self.mode = *mode;
                self.plan_path.clone_from(plan_path);
            }
            Event::ProcessListChanged { processes } => {
                self.processes = processes
                    .iter()
                    .cloned()
                    .map(|process| (process.id, process))
                    .collect();
            }
            Event::Usage {
                provider,
                account,
                usage,
                context_window,
                compaction_threshold,
                ..
            } => {
                self.usage.insert(
                    (provider.clone(), account.clone()),
                    UsageState {
                        usage: usage.clone(),
                        context_window: *context_window,
                        compaction_threshold: *compaction_threshold,
                    },
                );
                if self.selected_target.as_ref().is_some_and(|target| {
                    target.provider == *provider && target.account == *account
                }) {
                    self.context_tokens = Some(usage.input_tokens);
                    if compaction_threshold.is_some() {
                        self.compaction_threshold = *compaction_threshold;
                    }
                }
            }
            Event::TaskStarted { id } => {
                self.active = Some(*id);
                self.retry = None;
            }
            Event::TaskDone { id, .. } if self.active == Some(*id) => {
                self.active = None;
                self.retry = None;
            }
            Event::Retrying {
                id,
                attempt,
                max_attempts,
                delay_ms,
                reason,
                resets_at,
            } => {
                self.retry = Some(RetryState {
                    id: *id,
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    until: tokio::time::Instant::now()
                        + std::time::Duration::from_millis(*delay_ms),
                    reason: reason.clone(),
                    resets_at: *resets_at,
                });
            }
            Event::TextDelta { id, .. } | Event::ToolStarted { id, .. }
                if self.active == Some(*id) =>
            {
                self.retry = None;
            }
            _ => {}
        }
    }

    pub(crate) fn record_op(&mut self, id: TaskId, op: &Op) {
        self.transcript.record_op(id, op);
        if matches!(op, Op::ResolvePlan { .. }) {
            self.plan = None;
        }
    }

    pub(crate) fn fanout(&mut self, frame: &ServerFrame) {
        self.subscribers.retain(|sub| {
            if sub.sender.try_send(frame.clone()).is_ok() {
                true
            } else {
                tracing::warn!(
                    session = self.id.0,
                    client = sub.client.0,
                    "closing lagged subscriber"
                );
                sub.lagged.cancel();
                false
            }
        });
    }

    pub(crate) fn record_and_fanout(&mut self, event: Event) -> Option<PersistEvent> {
        update_state_from_event(&mut self.state, &event);
        self.transcript.apply(&event);
        match &event {
            Event::AskStarted { call, .. } => {
                self.open_asks += 1;
                self.asks.insert(*call, event.clone());
            }
            Event::AskDismissed { call, .. } => {
                self.open_asks = self.open_asks.saturating_sub(1);
                self.asks.remove(call);
            }
            Event::SubagentStarted { id, .. } => {
                self.subagents.insert(*id, event.clone());
            }
            Event::SubagentDone { id, .. } => {
                self.subagents.remove(id);
            }
            Event::PlanProposed { .. } => self.plan = Some(event.clone()),
            Event::ProcessStarted {
                process,
                command,
                watched,
            } => {
                self.live_processes += 1;
                self.processes.insert(
                    *process,
                    ProcessInfo {
                        id: *process,
                        command: command.clone(),
                        state: ProcessState::Running,
                        watched: *watched,
                        exit_code: None,
                    },
                );
            }
            Event::ProcessExited { process, code, .. } => {
                self.live_processes = self.live_processes.saturating_sub(1);
                if let Some(info) = self.processes.get_mut(process) {
                    info.state = ProcessState::Exited;
                    info.exit_code = *code;
                }
            }
            Event::Usage { usage, .. } => {
                self.tokens = self
                    .tokens
                    .saturating_add(u64::from(usage.input_tokens))
                    .saturating_add(u64::from(usage.output_tokens));
            }
            Event::ConversationBound { conversation_id } => {
                self.conversation_id = Some(*conversation_id);
            }
            _ => {}
        }
        self.cache_state_event(&event);
        if let Event::ConversationRestored {
            target,
            context_tokens,
            compaction_threshold,
            ..
        } = &event
        {
            self.restore_target = Some(target.clone());
            self.context_tokens = *context_tokens;
            self.compaction_threshold = *compaction_threshold;
            self.awaits_restore = false;
            self.ready.notify_waiters();
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.log.len() >= MAX_RETAINED_EVENTS {
            self.log.pop_front();
        }
        let prompt = prompt_action(&event);
        let conversation_id = self.conversation_id;
        let frame = ServerFrame::Event {
            session: self.id,
            seq,
            event: event.clone(),
        };
        self.log.push_back((seq, event));
        self.fanout(&frame);
        conversation_id.map(|conversation_id| PersistEvent {
            conversation_id,
            prompt,
        })
    }

    pub(crate) fn presence(&self) -> Vec<ClientId> {
        self.subscribers.iter().map(|s| s.client).collect()
    }

    pub(crate) fn subscribe_ready(&self) -> bool {
        if self.awaits_restore {
            self.restore_target.is_some()
        } else {
            self.state_ready || self.restore_target.is_some()
        }
    }

    pub(crate) fn build_snapshot(&self) -> ServerFrame {
        let rate_limits = self
            .rate_limits
            .iter()
            .map(
                |((provider, account), (snapshot, cached_at))| RateLimitEntry {
                    provider: provider.clone(),
                    account: account.clone(),
                    snapshot: snapshot.clone(),
                    cached_at: *cached_at,
                },
            )
            .collect();
        let usage = self
            .usage
            .iter()
            .map(|((provider, account), state)| UsageEntry {
                provider: provider.clone(),
                account: account.clone(),
                usage: state.usage.clone(),
                context_window: state.context_window,
                compaction_threshold: state.compaction_threshold,
            })
            .collect();
        let mut processes: Vec<_> = self.processes.values().cloned().collect();
        processes.sort_by_key(|process| process.id);
        let retry = self.retry.as_ref().map(|retry| RetryEntry {
            id: retry.id,
            attempt: retry.attempt,
            max_attempts: retry.max_attempts,
            delay_ms: u64::try_from(
                retry
                    .until
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            reason: retry.reason.clone(),
            resets_at: retry.resets_at,
        });
        let mut pending = Vec::new();
        if let Some(conversation_id) = self.conversation_id {
            pending.push(Event::ConversationBound { conversation_id });
        }
        pending.extend(self.transcript.pending_events());
        let mut subagents: Vec<_> = self.subagents.iter().collect();
        subagents.sort_by_key(|(id, _)| **id);
        pending.extend(subagents.into_iter().map(|(_, event)| event.clone()));
        let mut asks: Vec<_> = self.asks.iter().collect();
        asks.sort_by_key(|(call, _)| **call);
        pending.extend(asks.into_iter().map(|(_, event)| event.clone()));
        pending.extend(self.plan.iter().cloned());
        ServerFrame::Snapshot {
            session: self.id,
            watermark: self.next_seq,
            target: Box::new(
                self.selected_target
                    .clone()
                    .or_else(|| self.restore_target.clone()),
            ),
            transcript: self.transcript.entries.clone(),
            pending,
            context_tokens: self.context_tokens,
            compaction_threshold: self.compaction_threshold,
            skills: self.skills.clone(),
            accounts: self.accounts.clone(),
            model_list: self.model_list.clone(),
            selected: Box::new(self.selected_target.clone()),
            rate_limits,
            mode: ModeEntry {
                mode: self.mode,
                plan_path: self.plan_path.clone(),
            },
            processes,
            usage,
            active: self.active,
            retry: Box::new(retry),
        }
    }

    pub(crate) fn evictable(&self) -> bool {
        self.subscribers.is_empty()
            && self.pending_attaches == 0
            && self.open_asks == 0
            && self.live_processes == 0
            && matches!(self.state, SessionLiveState::Idle {})
    }
}

const MAX_RETAINED_EVENTS: usize = 4096;

fn update_state_from_event(state: &mut SessionLiveState, event: &Event) {
    match event {
        Event::TaskStarted { .. } | Event::AskDismissed { .. } => {
            *state = SessionLiveState::Active {};
        }
        Event::AskStarted { .. } => *state = SessionLiveState::WaitingOnAsk {},
        Event::TaskDone { .. } => *state = SessionLiveState::Idle {},
        _ => {}
    }
}

fn prompt_action(event: &Event) -> Option<PromptAction> {
    match event {
        Event::AskStarted {
            id,
            call,
            questions,
        } => Some(PromptAction::Open {
            call_id: format!("{}", call.0),
            kind: "ask".to_owned(),
            payload: serde_json::to_string(questions).unwrap_or_default(),
            task_id: id.0,
        }),
        Event::AskDismissed { call, .. } => Some(PromptAction::Close {
            call_id: format!("{}", call.0),
        }),
        _ => None,
    }
}

pub(crate) fn subscriber_map_remove(subs: &mut Vec<Subscriber>, client: ClientId) {
    subs.retain(|s| s.client != client);
}

pub(crate) fn subscriber_upsert(
    subs: &mut Vec<Subscriber>,
    client: ClientId,
    sender: mpsc::Sender<ServerFrame>,
    lagged: tokio_util::sync::CancellationToken,
) {
    if let Some(existing) = subs.iter_mut().find(|s| s.client == client) {
        existing.sender = sender;
        existing.lagged = lagged;
    } else {
        subs.push(Subscriber {
            client,
            sender,
            lagged,
        });
    }
}

pub(crate) type SessionTable = HashMap<SessionId, LiveSession>;

#[cfg(test)]
mod tests {
    use super::{
        PromptAction, SessionInner, Subscriber, prompt_action, subscriber_map_remove,
        subscriber_upsert,
    };
    use std::collections::HashMap;

    use goat_protocol::{AskQuestion, Event, TaskId, ToolCallId};
    use goat_wire::{ClientId, ServerFrame, SessionId, SessionLiveState};
    use tokio::sync::mpsc;

    fn blank_inner() -> SessionInner {
        let (ops, _ops_rx) = mpsc::channel(8);
        SessionInner {
            id: SessionId(1),
            cwd: "/tmp".to_owned(),
            created_at: 0,
            ops,
            log: std::collections::VecDeque::new(),
            next_seq: 0,
            next_task: 1,
            subscribers: Vec::new(),
            state: SessionLiveState::Idle {},
            transcript: super::LiveTranscript::default(),
            restore_target: None,
            context_tokens: None,
            compaction_threshold: None,
            tokens: 0,
            open_asks: 0,
            live_processes: 0,
            conversation_id: None,
            awaits_restore: false,
            ready: std::sync::Arc::new(tokio::sync::Notify::new()),
            resurrected: std::collections::HashSet::new(),
            pending_attaches: 0,
            skills: Vec::new(),
            accounts: Vec::new(),
            model_list: Vec::new(),
            selected_target: None,
            rate_limits: HashMap::new(),
            usage: HashMap::new(),
            mode: goat_protocol::Mode::Normal,
            plan_path: None,
            processes: HashMap::new(),
            active: None,
            retry: None,
            asks: HashMap::new(),
            subagents: HashMap::new(),
            plan: None,
            state_ready: false,
        }
    }

    #[test]
    fn pending_attach_blocks_eviction() {
        let mut inner = blank_inner();
        assert!(inner.evictable(), "idle + no subscribers + no pending");
        inner.pending_attaches += 1;
        assert!(
            !inner.evictable(),
            "an in-flight attach must keep the session alive"
        );
        inner.pending_attaches -= 1;
        assert!(inner.evictable());
    }

    #[test]
    fn live_process_blocks_eviction() {
        let mut inner = blank_inner();
        assert!(inner.evictable());
        inner.record_and_fanout(Event::ProcessStarted {
            process: goat_protocol::RunId(1),
            command: "pnpm dev".to_owned(),
            watched: false,
        });
        assert!(
            !inner.evictable(),
            "a live background process must keep the session alive after the window closes"
        );
        inner.record_and_fanout(Event::ProcessExited {
            process: goat_protocol::RunId(1),
            code: Some(0),
            reason: goat_protocol::ProcessExitReason::Natural,
        });
        assert!(
            inner.evictable(),
            "once the process exits the session may be evicted"
        );
    }

    #[test]
    fn upsert_replaces_sender_for_same_client() {
        let mut subs: Vec<Subscriber> = Vec::new();
        let (a, _ra) = mpsc::channel::<ServerFrame>(8);
        let (b, _rb) = mpsc::channel::<ServerFrame>(8);
        subscriber_upsert(
            &mut subs,
            ClientId(7),
            a,
            tokio_util::sync::CancellationToken::new(),
        );
        subscriber_upsert(
            &mut subs,
            ClientId(7),
            b,
            tokio_util::sync::CancellationToken::new(),
        );
        assert_eq!(subs.len(), 1);
        subscriber_map_remove(&mut subs, ClientId(7));
        assert!(subs.is_empty());
    }

    #[test]
    fn restored_watermark_skips_its_own_event() {
        let mut inner = blank_inner();
        inner.conversation_id = Some(1);
        let event = Event::ConversationRestored {
            target: goat_protocol::ModelTarget {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                account: "a".to_owned(),
                effort: None,
            },
            entries: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
        };
        inner.record_and_fanout(event);
        let ServerFrame::Snapshot { watermark, .. } = inner.build_snapshot() else {
            panic!("expected snapshot frame");
        };
        let restored_seq = inner.log.back().map(|(seq, _)| *seq).unwrap();
        assert_eq!(watermark, restored_seq + 1);
    }

    #[test]
    fn skills_changed_caches_and_marks_ready() {
        let mut inner = blank_inner();
        assert!(!inner.state_ready);
        inner.record_and_fanout(Event::SkillsChanged {
            skills: vec![goat_protocol::SkillInfo {
                name: "deploy".to_owned(),
                description: "ship it".to_owned(),
                command: None,
            }],
        });
        assert!(inner.state_ready);
        assert_eq!(inner.skills.len(), 1);
    }

    #[test]
    fn state_events_populate_snapshot() {
        let mut inner = blank_inner();
        inner.record_and_fanout(Event::AccountsChanged {
            providers: Vec::new(),
        });
        inner.record_and_fanout(Event::ModelListChanged {
            entries: Vec::new(),
        });
        inner.record_and_fanout(Event::SkillsChanged {
            skills: vec![goat_protocol::SkillInfo {
                name: "deploy".to_owned(),
                description: "ship it".to_owned(),
                command: None,
            }],
        });
        inner.record_and_fanout(Event::RateLimits {
            provider: "anthropic".to_owned(),
            account: "default".to_owned(),
            snapshot: goat_protocol::RateLimitSnapshot {
                windows: Vec::new(),
                representative: None,
            },
            cached_at: 42,
        });
        let ServerFrame::Snapshot {
            watermark,
            target,
            skills,
            rate_limits,
            ..
        } = inner.build_snapshot()
        else {
            panic!("expected snapshot frame");
        };
        assert!(
            target.is_none(),
            "new session snapshot has no restore target"
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(rate_limits.len(), 1);
        assert_eq!(watermark, inner.next_seq);
    }

    #[test]
    fn restored_transcript_stays_current_after_log_floor_advances() {
        let mut inner = blank_inner();
        inner.record_and_fanout(Event::ConversationRestored {
            target: goat_protocol::ModelTarget {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                account: "a".to_owned(),
                effort: None,
            },
            entries: vec![goat_protocol::TranscriptEntry::User {
                text: "restored".to_owned(),
                display: None,
                system: false,
                attachments: Vec::new(),
            }],
            context_tokens: Some(1),
            compaction_threshold: None,
        });
        for id in 0..=super::MAX_RETAINED_EVENTS {
            inner.record_and_fanout(Event::TextDone {
                id: TaskId(u64::try_from(id).unwrap()),
                text: format!("answer {id}"),
            });
        }
        let ServerFrame::Snapshot {
            watermark,
            transcript,
            ..
        } = inner.build_snapshot()
        else {
            panic!("expected snapshot frame");
        };
        assert_eq!(watermark, inner.next_seq);
        assert_eq!(transcript.len(), super::MAX_RETAINED_EVENTS + 2);
        assert!(matches!(
            transcript.last(),
            Some(goat_protocol::TranscriptEntry::Assistant { text })
                if text == &format!("answer {}", super::MAX_RETAINED_EVENTS)
        ));
    }

    #[test]
    fn never_restored_session_snapshots_live_history() {
        let mut inner = blank_inner();
        let target = goat_protocol::ModelTarget {
            provider: "p".to_owned(),
            model: "m".to_owned(),
            account: "a".to_owned(),
            effort: None,
        };
        inner.record_and_fanout(Event::ModelSelected {
            target: target.clone(),
        });
        inner.record_and_fanout(Event::UserMessage {
            id: TaskId(1),
            text: "hello".to_owned(),
            display: None,
            system: false,
            attachments: Vec::new(),
        });
        inner.record_and_fanout(Event::TextDone {
            id: TaskId(1),
            text: "world".to_owned(),
        });
        let ServerFrame::Snapshot {
            target: snapshot_target,
            transcript,
            ..
        } = inner.build_snapshot()
        else {
            panic!("expected snapshot frame");
        };
        assert_eq!(*snapshot_target, Some(target));
        assert_eq!(transcript.len(), 2);
    }

    #[tokio::test]
    async fn lagged_subscriber_is_cancelled_out_of_band() {
        let mut inner = blank_inner();
        let (sender, _receiver) = mpsc::channel(1);
        let lagged = tokio_util::sync::CancellationToken::new();
        subscriber_upsert(&mut inner.subscribers, ClientId(9), sender, lagged.clone());
        inner.fanout(&ServerFrame::Error {
            message: "first".to_owned(),
        });
        inner.fanout(&ServerFrame::Error {
            message: "second".to_owned(),
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), lagged.cancelled())
            .await
            .expect("lag cancellation must be observable");
        assert!(inner.subscribers.is_empty());
    }

    #[test]
    fn snapshot_matches_transcript_at_every_event_boundary() {
        let mut inner = blank_inner();
        let events = [
            Event::UserMessage {
                id: TaskId(1),
                text: "hello".to_owned(),
                display: None,
                system: false,
                attachments: Vec::new(),
            },
            Event::TextDelta {
                id: TaskId(1),
                chunk: "wo".to_owned(),
            },
            Event::TextDelta {
                id: TaskId(1),
                chunk: "rld".to_owned(),
            },
            Event::TextDone {
                id: TaskId(1),
                text: "world".to_owned(),
            },
        ];
        let expected = [
            vec![goat_protocol::TranscriptEntry::User {
                text: "hello".to_owned(),
                display: None,
                system: false,
                attachments: Vec::new(),
            }],
            vec![goat_protocol::TranscriptEntry::User {
                text: "hello".to_owned(),
                display: None,
                system: false,
                attachments: Vec::new(),
            }],
            vec![goat_protocol::TranscriptEntry::User {
                text: "hello".to_owned(),
                display: None,
                system: false,
                attachments: Vec::new(),
            }],
            vec![
                goat_protocol::TranscriptEntry::User {
                    text: "hello".to_owned(),
                    display: None,
                    system: false,
                    attachments: Vec::new(),
                },
                goat_protocol::TranscriptEntry::Assistant {
                    text: "world".to_owned(),
                },
            ],
        ];
        let pending = [
            Vec::new(),
            vec![Event::TextDelta {
                id: TaskId(1),
                chunk: "wo".to_owned(),
            }],
            vec![Event::TextDelta {
                id: TaskId(1),
                chunk: "world".to_owned(),
            }],
            Vec::new(),
        ];
        for ((event, expected), pending) in events.into_iter().zip(expected).zip(pending) {
            inner.record_and_fanout(event);
            let ServerFrame::Snapshot {
                watermark,
                transcript,
                pending: snapshot_pending,
                ..
            } = inner.build_snapshot()
            else {
                panic!("expected snapshot frame");
            };
            assert_eq!(watermark, inner.next_seq);
            assert_eq!(transcript, expected);
            assert_eq!(snapshot_pending, pending);
        }
    }

    #[test]
    fn log_is_bounded() {
        let mut inner = blank_inner();
        for _ in 0..(super::MAX_RETAINED_EVENTS + 50) {
            inner.record_and_fanout(Event::TextDelta {
                id: TaskId(0),
                chunk: "x".to_owned(),
            });
        }
        assert_eq!(inner.log.len(), super::MAX_RETAINED_EVENTS);
    }

    #[test]
    fn ask_started_maps_to_open_prompt() {
        let event = Event::AskStarted {
            id: TaskId(5),
            call: ToolCallId(9),
            questions: vec![AskQuestion {
                question: "Deploy?".to_owned(),
                options: Vec::new(),
                multiple: false,
            }],
        };
        match prompt_action(&event) {
            Some(PromptAction::Open {
                call_id,
                kind,
                task_id,
                ..
            }) => {
                assert_eq!(call_id, "9");
                assert_eq!(kind, "ask");
                assert_eq!(task_id, 5);
            }
            _ => panic!("expected open prompt"),
        }
    }

    #[test]
    fn ask_dismissed_maps_to_close() {
        let event = Event::AskDismissed {
            id: TaskId(5),
            call: ToolCallId(9),
        };
        match prompt_action(&event) {
            Some(PromptAction::Close { call_id }) => assert_eq!(call_id, "9"),
            _ => panic!("expected close prompt"),
        }
    }
}

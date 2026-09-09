use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use goat_client::{Attachment, Identity};
use goat_command::{Composer, Session, SessionSnapshot, Settings, UsageState, Viewport};
use goat_protocol::{
    AccountEntry, ConversationSummary, Event, LoginProvider, Mode, ModelEntry, ModelTarget,
    NotifyKind, Op, ProcessInfo, ProcessState, SkillInfo, TaskId, ToolCallId, TranscriptEntry,
    Usage,
};
use serde::Serialize;

use super::adapter::WebviewControls;

#[derive(Clone, Copy)]
pub enum TaskKind {
    Message,
    Shell,
    Compact,
}

impl TaskKind {
    pub fn from_op(op: &Op) -> Result<Self, String> {
        match op {
            Op::SubmitMessage { .. } => Ok(Self::Message),
            Op::SubmitShell { .. } => Ok(Self::Shell),
            Op::Compact { .. } => Ok(Self::Compact),
            _ => Err("operation does not submit a task".to_owned()),
        }
    }
}

#[derive(Default)]
struct SubmissionProgress {
    consumed: HashSet<TaskId>,
    finished: HashSet<TaskId>,
    failed: bool,
}

pub struct DesktopSession {
    pub session_id: u64,
    pub client_id: u64,
    pub conversation_id: Option<i64>,
    pub cwd: String,
    pub daemon: Identity,
    pub models: Vec<ModelEntry>,
    pub model: Option<ModelTarget>,
    pub models_loaded: bool,
    pub accounts: Vec<AccountEntry>,
    pub conversations: Vec<ConversationSummary>,
    pub mode: Mode,
    pub plan_path: Option<String>,
    pub active: Option<TaskId>,
    pub queued: Vec<TaskId>,
    pub processes: Vec<ProcessInfo>,
    pub skills: Vec<SkillInfo>,
    pub usage: UsageState,
    pub context_tokens: Option<u32>,
    pub compaction_threshold: Option<u32>,
    pub window_count: usize,
    pub workspace: Option<goat_worktree::Workspace>,
    pub pull_request: Option<goat_github::PrInfo>,
    pub notifications: Vec<(NotifyKind, String)>,
    context_windows: HashMap<(String, String), u32>,
    login_providers: Vec<LoginProvider>,
    login_status: Vec<Event>,
    subagents: HashSet<TaskId>,
    group_calls: HashSet<(TaskId, ToolCallId)>,
    shell_tasks: HashSet<TaskId>,
    submission: Option<SubmissionProgress>,
    transcript_entries: usize,
    streaming: bool,
    thinking: bool,
    tasks: AtomicU64,
    controls: WebviewControls,
    started: Instant,
}

#[derive(Serialize)]
pub struct StateView<'a> {
    session_id: u64,
    client_id: u64,
    conversation_id: Option<i64>,
    cwd: &'a str,
    model: &'a Option<ModelTarget>,
    models_loaded: bool,
    models: &'a [ModelEntry],
    accounts: &'a [AccountEntry],
    conversations: &'a [ConversationSummary],
    mode: Mode,
    plan_path: &'a Option<String>,
    busy: bool,
    active: Option<TaskId>,
    queued: &'a [TaskId],
    processes: &'a [ProcessInfo],
    skills: &'a [SkillInfo],
    usage: Vec<goat_api::UsageEntry>,
    rate_limits: Vec<goat_api::RateLimitEntry>,
    turn_tokens: u64,
    context_tokens: Option<u32>,
    context_window: Option<u32>,
    compaction_threshold: Option<u32>,
    window_count: usize,
    transcript_entries: usize,
    login_providers: &'a [LoginProvider],
    login_status: &'a [Event],
}

impl DesktopSession {
    pub fn new(attachment: &Attachment) -> Self {
        Self {
            session_id: attachment.session(),
            client_id: attachment.client_id,
            conversation_id: None,
            cwd: attachment.cwd.clone(),
            daemon: attachment.daemon.clone(),
            models: Vec::new(),
            model: None,
            models_loaded: false,
            accounts: Vec::new(),
            conversations: Vec::new(),
            mode: Mode::default(),
            plan_path: None,
            active: None,
            queued: Vec::new(),
            processes: Vec::new(),
            skills: Vec::new(),
            usage: UsageState::default(),
            context_tokens: None,
            compaction_threshold: None,
            window_count: 1,
            workspace: None,
            pull_request: None,
            notifications: Vec::new(),
            context_windows: HashMap::new(),
            login_providers: Vec::new(),
            login_status: Vec::new(),
            subagents: HashSet::new(),
            group_calls: HashSet::new(),
            shell_tasks: HashSet::new(),
            submission: None,
            transcript_entries: 0,
            streaming: false,
            thinking: false,
            tasks: AtomicU64::new(1),
            controls: WebviewControls,
            started: Instant::now(),
        }
    }

    pub fn rebind(&mut self, mut replacement: Self) {
        replacement.tasks = AtomicU64::new(self.tasks.load(Ordering::Relaxed));
        replacement.started = self.started;
        *self = replacement;
    }

    pub fn view(&self) -> StateView<'_> {
        let mut usage: Vec<_> = self
            .usage
            .last
            .iter()
            .map(|(key, usage)| goat_api::UsageEntry {
                provider: key.0.clone(),
                account: key.1.clone(),
                usage: usage.clone(),
                context_window: self.context_windows.get(key).copied(),
                compaction_threshold: self.compaction_threshold,
            })
            .collect();
        usage.sort_by(|a, b| (&a.provider, &a.account).cmp(&(&b.provider, &b.account)));
        let mut rate_limits: Vec<_> = self
            .usage
            .rate_limits
            .iter()
            .map(
                |((provider, account), (snapshot, cached_at))| goat_api::RateLimitEntry {
                    provider: provider.clone(),
                    account: account.clone(),
                    snapshot: snapshot.clone(),
                    cached_at: *cached_at,
                },
            )
            .collect();
        rate_limits.sort_by(|a, b| (&a.provider, &a.account).cmp(&(&b.provider, &b.account)));
        let context_window = self.model.as_ref().and_then(|target| {
            self.context_windows
                .get(&(target.provider.clone(), target.account.clone()))
                .copied()
                .or_else(|| {
                    self.models
                        .iter()
                        .find(|entry| {
                            entry.provider == target.provider && entry.model == target.model
                        })
                        .and_then(|entry| entry.context_window)
                })
        });
        StateView {
            session_id: self.session_id,
            client_id: self.client_id,
            conversation_id: self.conversation_id,
            cwd: &self.cwd,
            model: &self.model,
            models_loaded: self.models_loaded,
            models: &self.models,
            accounts: &self.accounts,
            conversations: &self.conversations,
            mode: self.mode,
            plan_path: &self.plan_path,
            busy: self.is_busy(),
            active: self.active,
            queued: &self.queued,
            processes: &self.processes,
            skills: &self.skills,
            usage,
            rate_limits,
            turn_tokens: self.usage.turn_tokens,
            context_tokens: self.context_tokens,
            context_window,
            compaction_threshold: self.compaction_threshold,
            window_count: self.window_count,
            transcript_entries: self.transcript_entries,
            login_providers: &self.login_providers,
            login_status: &self.login_status,
        }
    }

    pub fn normalize(&self, op: &mut Op) {
        match op {
            Op::SubmitMessage { id, .. }
            | Op::SubmitShell { id, .. }
            | Op::Interrupt { id }
            | Op::Answer { id, .. }
            | Op::Compact { id, .. }
            | Op::DequeueMessage { id } => {
                if id.0 == 0 {
                    *id = self.next_task();
                } else {
                    self.observe_task(*id);
                }
            }
            _ => {}
        }
    }

    pub fn submitting(&mut self) {
        self.submission = Some(SubmissionProgress::default());
    }

    pub fn submission_failed(&mut self) {
        self.submission = None;
    }

    pub fn submitted(&mut self, kind: TaskKind, id: TaskId) {
        let progress = self.submission.take().unwrap_or_default();
        let finished = progress.failed || progress.finished.contains(&id);
        if self.active.is_none() && !finished {
            self.active = Some(id);
        }
        match kind {
            TaskKind::Message if !finished && !progress.consumed.contains(&id) => {
                self.queued.push(id);
            }
            TaskKind::Shell => {
                if self.shell_tasks.insert(id) {
                    self.transcript_entries += 1;
                }
            }
            TaskKind::Message | TaskKind::Compact => {}
        }
    }

    pub fn event(&mut self, event: &Event) {
        self.catalog_event(event);
        self.turn_event(event);
        self.transcript_event(event);
    }

    fn catalog_event(&mut self, event: &Event) {
        match event {
            Event::ModelListChanged { entries } => {
                self.models.clone_from(entries);
                self.models_loaded = true;
            }
            Event::ModelSelected { target } => self.model = Some(target.clone()),
            Event::AccountsChanged { providers } => self.accounts.clone_from(providers),
            Event::ConversationsListed { conversations } => {
                self.conversations.clone_from(conversations);
            }
            Event::ModeChanged { mode, plan_path } => {
                self.mode = *mode;
                self.plan_path.clone_from(plan_path);
            }
            Event::SkillsChanged { skills } => self.skills.clone_from(skills),
            Event::ConversationBound { conversation_id } => {
                self.conversation_id = Some(*conversation_id);
            }
            Event::LoginProviders { providers } => self.login_providers.clone_from(providers),
            Event::LoginStatus { provider, .. } => {
                self.login_status.retain(|status| {
                    !matches!(status, Event::LoginStatus { provider: previous, .. } if previous == provider)
                });
                self.login_status.push(event.clone());
            }
            Event::ProcessListChanged { processes } => self.processes.clone_from(processes),
            Event::ProcessStarted {
                process,
                command,
                watched,
            } => {
                let info = ProcessInfo {
                    id: *process,
                    command: command.clone(),
                    state: ProcessState::Running,
                    watched: *watched,
                    exit_code: None,
                };
                if let Some(previous) = self.processes.iter_mut().find(|item| item.id == *process) {
                    *previous = info;
                } else {
                    self.processes.push(info);
                }
            }
            Event::ProcessExited { process, code, .. } => {
                if let Some(info) = self.processes.iter_mut().find(|item| item.id == *process) {
                    info.state = ProcessState::Exited;
                    info.exit_code = *code;
                }
            }
            Event::RateLimits {
                provider,
                account,
                snapshot,
                cached_at,
            } => {
                self.usage.rate_limits.insert(
                    (provider.clone(), account.clone()),
                    (snapshot.clone(), *cached_at),
                );
            }
            _ => {}
        }
    }

    fn turn_event(&mut self, event: &Event) {
        match event {
            Event::TaskStarted { id } => {
                self.active = Some(*id);
                self.usage.turn_tokens = 0;
            }
            Event::TaskDone { id, interrupted } => {
                if let Some(progress) = &mut self.submission {
                    progress.finished.insert(*id);
                }
                if self.active == Some(*id) {
                    self.active = None;
                }
                if *interrupted {
                    self.queued.clear();
                }
            }
            Event::Error { id, .. } if id.is_none_or(|id| !self.subagents.contains(&id)) => {
                if let Some(progress) = &mut self.submission {
                    if let Some(id) = id {
                        progress.finished.insert(*id);
                    } else {
                        progress.failed = true;
                    }
                }
                self.active = None;
                self.queued.clear();
            }
            Event::UserMessage { id, .. } | Event::MessageDequeued { id, .. } => {
                if let Some(progress) = &mut self.submission {
                    progress.consumed.insert(*id);
                }
                self.queued.retain(|queued| queued != id);
            }
            Event::SubagentStarted { id, .. } => {
                self.subagents.insert(*id);
            }
            Event::Usage {
                id,
                provider,
                account,
                usage,
                context_window,
                compaction_threshold,
            } if !self.subagents.contains(id) => {
                let key = (provider.clone(), account.clone());
                self.add_usage(key.clone(), usage);
                self.usage.last.insert(key.clone(), usage.clone());
                if let Some(window) = context_window {
                    self.context_windows.insert(key, *window);
                }
                if compaction_threshold.is_some() {
                    self.compaction_threshold = *compaction_threshold;
                }
                self.context_tokens = Some(usage.input_tokens);
            }
            Event::CompactionDone {
                id,
                ok: true,
                tokens_after,
                usage,
                ..
            } if !self.subagents.contains(id) => {
                if let Some(model) = &self.model {
                    let key = (model.provider.clone(), model.account.clone());
                    self.add_usage(key.clone(), usage);
                    self.usage.last.insert(
                        key,
                        Usage {
                            input_tokens: *tokens_after,
                            ..Usage::default()
                        },
                    );
                }
                self.context_tokens = Some(*tokens_after);
            }
            _ => {}
        }
    }

    fn transcript_event(&mut self, event: &Event) {
        match event {
            Event::ConversationRestored {
                target,
                entries,
                context_tokens,
                compaction_threshold,
            } => {
                self.active = None;
                self.subagents.clear();
                self.group_calls.clear();
                self.shell_tasks.clear();
                self.streaming = false;
                self.thinking = false;
                self.transcript_entries = entries
                    .iter()
                    .filter(|entry| !matches!(entry, TranscriptEntry::Thinking { text } if text.trim().is_empty()))
                    .count();
                self.usage.last.clear();
                self.usage.turn_tokens = 0;
                self.context_tokens = *context_tokens;
                self.compaction_threshold = *compaction_threshold;
                if let Some(tokens) = context_tokens {
                    self.usage.last.insert(
                        (target.provider.clone(), target.account.clone()),
                        Usage {
                            input_tokens: *tokens,
                            ..Usage::default()
                        },
                    );
                }
                self.model = Some(target.clone());
            }
            Event::ThinkingDelta { id, chunk } if !self.subagents.contains(id) => {
                self.thinking |= !chunk.trim().is_empty();
            }
            Event::TextDelta { id, .. } if !self.subagents.contains(id) => {
                self.flush_thinking();
                self.streaming = true;
            }
            Event::TextDone { id, .. } if !self.subagents.contains(id) => {
                self.flush_thinking();
                self.streaming = false;
                self.transcript_entries += 1;
            }
            Event::UserMessage { .. } => {
                self.flush_thinking();
                self.transcript_entries += 1;
            }
            Event::ToolStarted { id, call }
                if !self.subagents.contains(id) && !self.group_calls.contains(&(*id, call.id)) =>
            {
                self.flush_thinking();
                self.transcript_entries += 1;
            }
            Event::SubagentGroupStarted { id, group, members } => {
                if !self.subagents.contains(id) && self.group_calls.insert((*id, *group)) {
                    self.flush_thinking();
                    self.transcript_entries += 1;
                }
                for member in members {
                    self.group_calls.insert((*id, member.call));
                }
            }
            Event::ShellDone { id, .. } if !self.subagents.contains(id) => {
                if self.shell_tasks.insert(*id) {
                    self.transcript_entries += 1;
                }
            }
            Event::CompactionDone { id, ok: true, .. } if !self.subagents.contains(id) => {
                self.transcript_entries += 1;
            }
            Event::Retrying { id, .. } if !self.subagents.contains(id) => {
                self.streaming = false;
            }
            Event::TaskDone { .. } => self.finish_stream(),
            Event::Error { id, .. } if id.is_none_or(|id| !self.subagents.contains(&id)) => {
                self.finish_stream();
                self.transcript_entries += 1;
            }
            _ => {}
        }
    }

    fn add_usage(&mut self, key: (String, String), usage: &Usage) {
        let input = u64::from(usage.input_tokens);
        let output = u64::from(usage.output_tokens);
        self.usage.turn_tokens = self.usage.turn_tokens.saturating_add(input + output);
        let total = self.usage.total.entry(key).or_default();
        total.0 = total.0.saturating_add(input);
        total.1 = total.1.saturating_add(output);
    }

    fn flush_thinking(&mut self) {
        if std::mem::take(&mut self.thinking) {
            self.transcript_entries += 1;
        }
    }

    fn finish_stream(&mut self) {
        self.flush_thinking();
        if std::mem::take(&mut self.streaming) {
            self.transcript_entries += 1;
        }
    }

    fn next_task(&self) -> TaskId {
        TaskId(self.tasks.fetch_add(1, Ordering::Relaxed))
    }

    fn observe_task(&self, id: TaskId) {
        self.tasks
            .fetch_max(id.0.saturating_add(1), Ordering::Relaxed);
    }
}

impl Session for DesktopSession {
    fn models(&self) -> &[ModelEntry] {
        &self.models
    }

    fn current_model(&self) -> Option<&ModelTarget> {
        self.model.as_ref()
    }

    fn conversations(&self) -> &[ConversationSummary] {
        &self.conversations
    }

    fn usage(&self) -> &UsageState {
        &self.usage
    }

    fn mode(&self) -> Mode {
        self.mode
    }

    fn accounts(&self) -> &[AccountEntry] {
        &self.accounts
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            session_id: Some(self.session_id),
            client_id: Some(self.client_id),
            conversation_id: self.conversation_id,
            daemon: Some(self.daemon.clone()),
            model: self.model.clone(),
            models_loaded: self.models_loaded,
            mode: self.mode,
            plan_path: self.plan_path.clone(),
            cwd: self.cwd.clone(),
            remote: None,
            workspace: self.workspace.clone(),
            pull_request: self.pull_request.clone(),
            window_count: self.window_count,
            queued_count: self.queued.len(),
            process_count: self.processes.len(),
            skill_count: self.skills.len(),
            transcript_entries: self.transcript_entries,
            mouse_capture: false,
            dark_theme: true,
            log_path: goat_config::log_dir()
                .map(|dir| dir.join("desktop.log").display().to_string()),
            started: self.started,
        }
    }

    fn is_busy(&self) -> bool {
        self.active.is_some()
    }

    fn queued_len(&self) -> usize {
        self.queued.len()
    }

    fn settings(&mut self) -> &mut dyn Settings {
        &mut self.controls
    }

    fn composer(&mut self) -> &mut dyn Composer {
        &mut self.controls
    }

    fn viewport(&mut self) -> &mut dyn Viewport {
        &mut self.controls
    }

    fn notify(&mut self, kind: NotifyKind, message: String) {
        self.notifications.push((kind, message));
    }

    fn allocate_task(&mut self) -> TaskId {
        self.next_task()
    }
}

mod engine;
mod keys;

use std::{collections::HashMap, path::Path, time::Duration};

use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind, MouseEventKind};
use futures::StreamExt;
use goat_client::Identity;
use goat_command::{
    InputOutcome, Screen, ScreenOutcome, Session, SessionSnapshot, Settings, UsageState, Viewport,
};
use goat_commands::{CommandEffect, CommandRegistry};
use goat_protocol::{
    AccountEntry, Effort, Event as EngineEvent, ModelEntry, ModelTarget, NotifyKind, Op, TaskId,
    ToolCallId,
};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    command::{CommandMenu, CommandMenuContext, RuntimeChoice, RuntimeChoiceGroup},
    composer::Composer,
    files::FileMenu,
    highlight::SyntectHighlighter,
    native_screen::{AskScreen, CommandMenuScreen, FileMenuScreen, ImageZoomScreen},
    symbols,
    theme::Theme,
    transcript::Transcript,
    tui, view,
};

pub(crate) struct SubagentRunView {
    pub(crate) subagent_type: String,
    pub(crate) label: String,
    pub(crate) id: TaskId,
    pub(crate) parent: TaskId,
    pub(crate) call: ToolCallId,
    pub(crate) transcript: Transcript,
    pub(crate) done: Option<bool>,
    pub(crate) tools: u64,
    pub(crate) tokens: u64,
    pub(crate) started_at: std::time::Instant,
    pub(crate) finished_at: Option<std::time::Instant>,
}

pub(crate) struct ProcessRunView {
    pub(crate) id: goat_protocol::RunId,
    pub(crate) command: String,
    pub(crate) state: goat_protocol::ProcessState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) transcript: Transcript,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainView {
    Live,
    Subagent(TaskId),
    Process(goat_protocol::RunId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunTarget {
    Subagent(TaskId),
    Process(goat_protocol::RunId),
}

impl RunTarget {
    fn view(self) -> MainView {
        match self {
            RunTarget::Subagent(id) => MainView::Subagent(id),
            RunTarget::Process(id) => MainView::Process(id),
        }
    }
}

pub(crate) enum Overlay {
    None,
    Screen(Box<dyn Screen>),
    Runs(usize),
}

const TICK: Duration = Duration::from_millis(120);
const QUIT_ARM_TICKS: u16 = 25;
const CLEAR_ARM_TICKS: u16 = 25;
const BRANCH_POLL_TICKS: u16 = 8;
const PR_POLL_TICKS: u16 = 500;

pub(crate) enum AppEvent {
    Input(CtEvent),
    Tick,
    Engine(EngineEvent),
    AttachmentPaste {
        text: String,
        result: Result<Vec<goat_protocol::InputAttachment>, String>,
        fallback: bool,
    },
    ClipboardImage(Result<goat_protocol::InputAttachment, String>),
    EngineClosed,
    Presence(usize),
    PrStatus {
        branch: String,
        pr: Option<goat_github::PrInfo>,
    },
}

struct PrLookup {
    repo_root: std::path::PathBuf,
    branch: String,
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub(crate) theme: Theme,
    pub(crate) terminal_bg: Option<ratatui::style::Color>,
    pub(crate) transcript: Transcript,
    pub(crate) composer: Composer,
    pub(crate) highlighter: SyntectHighlighter,
    pub(crate) cwd: String,
    pub(crate) remote: Option<String>,
    git_workspace: Option<goat_worktree::Workspace>,
    pr: Option<goat_github::PrInfo>,
    pr_branch: Option<String>,
    pr_inflight: bool,
    pr_poll: u16,
    pr_enabled: bool,
    pub(crate) next_task: u64,
    pub(crate) window_count: usize,
    pub(crate) spinner: usize,
    pub(crate) quit_arm: Option<u16>,
    pub(crate) clear_arm: Option<u16>,
    pub(crate) rewind_arm: Option<u16>,
    branch_poll: u16,
    pub(crate) queued: Vec<(
        TaskId,
        String,
        Option<String>,
        Vec<goat_protocol::InputAttachment>,
    )>,
    pub(crate) should_quit: bool,
    pub(crate) exit_requested: bool,
    pub(crate) dirty: bool,
    pub(crate) scroll: usize,
    pub(crate) follow: bool,
    pub(crate) viewport_rows: u16,
    pub(crate) selection: Option<crate::select::Selection>,
    pub(crate) selection_version: u64,
    pub(crate) transcript_area: ratatui::layout::Rect,
    pub(crate) pending_copy: Option<String>,
    pub(crate) pending_open: Option<String>,
    pub(crate) last_click: Option<(std::time::Instant, usize, u16)>,
    pub(crate) models: Vec<ModelEntry>,
    pub(crate) models_loaded: bool,
    pub(crate) threads: Vec<goat_protocol::ThreadSummary>,
    pub(crate) model: Option<ModelTarget>,
    pub(crate) mode: goat_protocol::Mode,
    pub(crate) plan_path: Option<String>,
    pub(crate) overlay: Overlay,
    pub(crate) pending: PendingState,
    pub(crate) account_entries: Vec<AccountEntry>,
    pub(crate) mouse_capture: bool,
    pub(crate) computer_use: bool,
    pub(crate) browser: bool,
    pub(crate) commands: CommandRegistry,
    command_menu: std::sync::Weak<std::sync::Mutex<CommandMenu>>,
    file_menu: std::sync::Weak<std::sync::Mutex<FileMenu>>,
    pub(crate) toasts: Vec<crate::toast::Toast>,
    pub(crate) subagent_runs: Vec<SubagentRunView>,
    pub(crate) process_runs: Vec<ProcessRunView>,
    pub(crate) main_view: MainView,
    pub(crate) turn: TurnStatus,
    pub(crate) usage: UsageState,
    pub(crate) context_window: HashMap<(String, String), u32>,
    pub(crate) compaction_threshold: Option<u32>,
    pub(crate) focused: bool,
    pub(crate) notification_pending: Option<crate::notification::Notification>,
    pub(crate) picker: Option<std::sync::Arc<ratatui_image::picker::Picker>>,
    pub(crate) processes: Vec<goat_protocol::ProcessInfo>,
    pub(crate) files: Vec<String>,
    pub(crate) files_loaded: bool,
    pub(crate) outbox: Vec<Op>,
    pub(crate) session_id: Option<u64>,
    pub(crate) client_id: Option<u64>,
    pub(crate) thread_id: Option<i64>,
    pub(crate) daemon: Option<Identity>,
    pub(crate) started: std::time::Instant,
}

#[derive(Default)]
pub(crate) struct PendingState {
    pub(crate) ask: Option<AskScreen>,
}

#[derive(Default)]
pub(crate) struct TurnStatus {
    pub(crate) active: Option<TaskId>,
    pub(crate) active_shell: bool,
    pub(crate) thinking: bool,
    pub(crate) task_start: Option<std::time::Instant>,
    pub(crate) retry: Option<RetryState>,
    pub(crate) compacting: bool,
}

pub struct Origin {
    pub cwd: String,
    pub remote: Option<String>,
    pub session: Option<u64>,
    pub client: Option<u64>,
    pub daemon: Option<Identity>,
}

impl Origin {
    #[must_use]
    pub fn local(cwd: String) -> Self {
        Self {
            cwd,
            remote: None,
            session: None,
            client: None,
            daemon: None,
        }
    }

    #[must_use]
    pub fn remote(cwd: String, name: String) -> Self {
        Self {
            cwd,
            remote: Some(name),
            session: None,
            client: None,
            daemon: None,
        }
    }

    #[must_use]
    pub fn with_attachment(mut self, attachment: &goat_client::Attachment) -> Self {
        self.session = Some(attachment.session());
        self.client = Some(attachment.client_id);
        self.daemon = Some(attachment.daemon.clone());
        self
    }
}

pub(crate) struct RetryState {
    pub(crate) attempt: u32,
    pub(crate) max_attempts: u32,
    pub(crate) reason: String,
    pub(crate) until: std::time::Instant,
}

impl App {
    pub(crate) fn set_terminal_bg(&mut self, bg: Option<ratatui::style::Color>) {
        self.terminal_bg = bg;
        self.theme = self.theme.with_base(bg);
        self.transcript.invalidate();
        for run in &mut self.subagent_runs {
            run.transcript.invalidate();
        }
    }

    pub(crate) fn new(theme: Theme, origin: &Origin) -> Self {
        let remote = origin.remote.is_some();
        let cwd = match &origin.remote {
            Some(name) => format!("{name}:{}", origin.cwd),
            None => shorten_home(std::path::Path::new(&origin.cwd)),
        };
        let git_workspace = (!remote)
            .then(|| goat_worktree::workspace(std::path::Path::new(&origin.cwd)).ok())
            .flatten();
        let cfg = goat_config::Config::load();
        Self {
            theme,
            terminal_bg: None,
            transcript: Transcript::default(),
            composer: Composer::default(),
            highlighter: SyntectHighlighter::new(),
            cwd,
            remote: origin.remote.clone(),
            git_workspace,
            pr: None,
            pr_branch: None,
            pr_inflight: false,
            pr_poll: 0,
            pr_enabled: !remote && goat_github::gh_available(),
            next_task: 1,
            window_count: 1,
            spinner: 0,
            quit_arm: None,
            clear_arm: None,
            rewind_arm: None,
            branch_poll: BRANCH_POLL_TICKS,
            queued: Vec::new(),
            should_quit: false,
            exit_requested: false,
            dirty: true,
            scroll: 0,
            follow: true,
            viewport_rows: 0,
            selection: None,
            selection_version: 0,
            transcript_area: ratatui::layout::Rect::default(),
            pending_copy: None,
            pending_open: None,
            last_click: None,
            models: Vec::new(),
            models_loaded: false,
            threads: Vec::new(),
            model: None,
            mode: goat_protocol::Mode::Normal,
            plan_path: None,
            overlay: Overlay::None,
            pending: PendingState::default(),
            account_entries: Vec::new(),
            mouse_capture: cfg.mouse_capture_enabled,
            computer_use: cfg.computer_use_enabled,
            browser: cfg.browser_enabled,
            commands: CommandRegistry::builtin(),
            command_menu: std::sync::Weak::new(),
            file_menu: std::sync::Weak::new(),
            toasts: Vec::new(),
            subagent_runs: Vec::new(),
            process_runs: Vec::new(),
            main_view: MainView::Live,
            turn: TurnStatus::default(),
            usage: UsageState::default(),
            context_window: HashMap::new(),
            compaction_threshold: None,
            focused: true,
            notification_pending: None,
            picker: None,
            processes: Vec::new(),
            files: Vec::new(),
            files_loaded: false,
            outbox: Vec::new(),
            session_id: origin.session,
            client_id: origin.client,
            thread_id: None,
            daemon: origin.daemon.clone(),
            started: std::time::Instant::now(),
        }
    }

    pub(crate) fn update(&mut self, event: AppEvent) -> Vec<Op> {
        let mut ops = self.reduce(event);
        ops.append(&mut self.outbox);
        ops
    }

    fn reduce(&mut self, event: AppEvent) -> Vec<Op> {
        match event {
            AppEvent::Tick => {
                let screen_ops = self.tick_screen();
                if self.turn.active.is_some() {
                    self.spinner = self.spinner.wrapping_add(1);
                    self.dirty = true;
                }
                if let Some(ticks) = &mut self.quit_arm {
                    *ticks = ticks.saturating_sub(1);
                    if *ticks == 0 {
                        self.quit_arm = None;
                        self.dirty = true;
                    }
                }
                if let Some(ticks) = &mut self.clear_arm {
                    *ticks = ticks.saturating_sub(1);
                    if *ticks == 0 {
                        self.clear_arm = None;
                        self.dirty = true;
                    }
                }
                if let Some(ticks) = &mut self.rewind_arm {
                    *ticks = ticks.saturating_sub(1);
                    if *ticks == 0 {
                        self.rewind_arm = None;
                        self.dirty = true;
                    }
                }
                if crate::toast::tick(&mut self.toasts) {
                    self.dirty = true;
                }
                self.branch_poll = self.branch_poll.saturating_sub(1);
                if self.branch_poll == 0 {
                    self.branch_poll = BRANCH_POLL_TICKS;
                    self.refresh_git_branch();
                }
                self.pr_poll = self.pr_poll.saturating_sub(1);
                screen_ops
            }
            AppEvent::PrStatus { branch, pr } => {
                self.pr_inflight = false;
                if self.git_workspace.as_ref().map(|w| w.git_branch.as_str())
                    == Some(branch.as_str())
                {
                    self.pr = pr;
                    self.pr_branch = Some(branch);
                    self.dirty = true;
                }
                Vec::new()
            }
            AppEvent::Input(CtEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                let ops = self.on_key(key);
                self.promote_pending_ask();
                ops
            }
            AppEvent::Input(CtEvent::Paste(text)) => {
                if let Some(ops) = self.handle_screen_input(&CtEvent::Paste(text.clone())) {
                    return ops;
                }
                match &mut self.overlay {
                    _ => {
                        match crate::attachment::attachments_from_paste(&text) {
                            Ok(attachments) => self.composer.push_attachments(attachments),
                            Err(
                                crate::attachment::AttachError::NotImages
                                | crate::attachment::AttachError::Empty,
                            ) => {
                                self.composer.insert_str(&text);
                            }
                            Err(err) => self.push_toast(NotifyKind::Error, err.to_string()),
                        }
                        self.update_command_menu();
                    }
                }
                self.dirty = true;
                Vec::new()
            }
            AppEvent::Input(CtEvent::Resize(..)) => {
                self.dirty = true;
                Vec::new()
            }
            AppEvent::Input(CtEvent::Mouse(mouse)) => {
                if let Some(ops) = self.handle_screen_input(&CtEvent::Mouse(mouse)) {
                    return ops;
                }
                self.on_mouse(mouse);
                Vec::new()
            }
            AppEvent::Input(CtEvent::FocusGained) => {
                self.focused = true;
                Vec::new()
            }
            AppEvent::Input(CtEvent::FocusLost) => {
                self.focused = false;
                Vec::new()
            }
            AppEvent::Input(_) => Vec::new(),
            AppEvent::Engine(event) => {
                let mut ops = self.notify_screen(&event);
                ops.extend(self.on_engine(event));
                self.promote_pending_ask();
                self.dirty = true;
                ops
            }
            AppEvent::AttachmentPaste {
                text,
                result,
                fallback,
            } => {
                match result {
                    Ok(attachments) => self.composer.push_attachments(attachments),
                    Err(_message) if fallback => self.composer.insert_paste(&text),
                    Err(message) => self.push_toast(NotifyKind::Error, message),
                }
                self.update_command_menu();
                self.dirty = true;
                Vec::new()
            }
            AppEvent::ClipboardImage(result) => {
                match result {
                    Ok(attachment) => self.composer.push_attachment(attachment),
                    Err(message) => self.push_toast(NotifyKind::Error, message),
                }
                self.update_command_menu();
                self.dirty = true;
                Vec::new()
            }
            AppEvent::EngineClosed => {
                self.should_quit = true;
                Vec::new()
            }
            AppEvent::Presence(count) => {
                if self.window_count != count {
                    self.window_count = count;
                    self.dirty = true;
                }
                Vec::new()
            }
        }
    }

    pub(crate) fn dispatch_slash_command(&mut self, raw: &str) -> Vec<Op> {
        let commands = std::mem::take(&mut self.commands);
        let effect = commands.resolve_line(raw, self);
        self.commands = commands;
        self.apply_command_effect(effect)
    }

    pub(crate) fn apply_command_effect(&mut self, effect: CommandEffect) -> Vec<Op> {
        self.dirty = true;
        match effect {
            CommandEffect::Show(mut screen) => {
                let outcome = screen.tick();
                self.apply_screen_outcome(screen, outcome)
            }
            CommandEffect::Dispatch(ops) => {
                for op in &ops {
                    match op {
                        Op::SetMode { mode } => self.mode = *mode,
                        Op::Clear {} => {
                            self.transcript.clear();
                            self.reset_subagents();
                            self.turn = TurnStatus::default();
                            self.clear_ctx_indicator();
                            self.scroll = 0;
                            self.follow = true;
                        }
                        _ => {}
                    }
                }
                ops
            }
            CommandEffect::Submit { display, prompt } => self.submit_command(display, prompt),
            CommandEffect::Noop => Vec::new(),
            CommandEffect::Quit => {
                self.exit_requested = true;
                self.should_quit = true;
                Vec::new()
            }
        }
    }

    fn apply_screen_outcome(
        &mut self,
        mut screen: Box<dyn Screen>,
        mut outcome: ScreenOutcome,
    ) -> Vec<Op> {
        let mut ops = Vec::new();
        loop {
            match outcome {
                ScreenOutcome::Continue => {
                    if matches!(self.overlay, Overlay::None) {
                        self.overlay = Overlay::Screen(screen);
                    }
                    return ops;
                }
                ScreenOutcome::Close => return ops,
                ScreenOutcome::Effect(effect) => {
                    ops.extend(self.apply_command_effect(effect));
                    if !matches!(self.overlay, Overlay::None) {
                        return ops;
                    }
                    outcome = screen.tick();
                }
            }
        }
    }

    fn handle_screen_input(&mut self, event: &CtEvent) -> Option<Vec<Op>> {
        let mut screen = match std::mem::replace(&mut self.overlay, Overlay::None) {
            Overlay::Screen(screen) => screen,
            overlay => {
                self.overlay = overlay;
                return None;
            }
        };
        match screen.handle_input(event, self) {
            InputOutcome::Ignored => {
                self.overlay = Overlay::Screen(screen);
                None
            }
            InputOutcome::Handled(outcome) => {
                let ops = self.apply_screen_outcome(screen, outcome);
                if self.command_menu.upgrade().is_some() {
                    self.update_command_menu();
                }
                Some(ops)
            }
        }
    }

    fn tick_screen(&mut self) -> Vec<Op> {
        let mut screen = match std::mem::replace(&mut self.overlay, Overlay::None) {
            Overlay::Screen(screen) => screen,
            overlay => {
                self.overlay = overlay;
                return Vec::new();
            }
        };
        let outcome = screen.tick();
        self.apply_screen_outcome(screen, outcome)
    }

    fn notify_screen(&mut self, event: &EngineEvent) -> Vec<Op> {
        let mut screen = match std::mem::replace(&mut self.overlay, Overlay::None) {
            Overlay::Screen(screen) => screen,
            overlay => {
                self.overlay = overlay;
                return Vec::new();
            }
        };
        let outcome = screen.on_event(event, self);
        self.apply_screen_outcome(screen, outcome)
    }

    pub(crate) fn request_rewind(&mut self) -> Vec<Op> {
        self.rewind_arm = None;
        if self.turn.active.is_some() || !self.queued.is_empty() {
            self.push_toast(
                NotifyKind::Info,
                "finish or interrupt the current task before rewinding".to_owned(),
            );
            Vec::new()
        } else {
            self.apply_command_effect(CommandEffect::Show(Box::new(
                goat_commands::RewindScreen::new(Vec::new()),
            )))
        }
    }

    pub(crate) fn submit(&mut self) -> Vec<Op> {
        if self.composer.is_empty() {
            return Vec::new();
        }
        if self.composer.shell() {
            if self.composer.text().trim().is_empty() {
                return Vec::new();
            }
            if self.turn.active.is_some() {
                self.push_toast(
                    NotifyKind::Info,
                    "finish or interrupt the task before running a shell command".to_owned(),
                );
                return Vec::new();
            }
            let command = self.composer.take();
            return self.submit_shell(command);
        }
        let mut attachments = self.composer.take_attachments();
        let text = self.composer.take();
        let (text, promoted) = crate::attachment::extract_image_paths(&text);
        attachments.extend(promoted);
        let trimmed = text.trim();
        if trimmed.is_empty() && attachments.is_empty() {
            return Vec::new();
        }
        if trimmed.starts_with('/') {
            let cmd = trimmed.to_owned();
            if slash_command_name(&cmd).is_some_and(|name| self.commands.contains(name)) {
                return self.dispatch_slash_command(&cmd);
            }
        }
        if !attachments.is_empty() && !self.current_model_supports_images() {
            self.composer.set_plain_text(&text);
            self.composer.push_attachments(attachments);
            self.push_toast(
                NotifyKind::Error,
                "current model does not support image input".to_owned(),
            );
            self.dirty = true;
            return Vec::new();
        }
        self.submit_text_with_attachments(text, attachments)
    }

    pub(crate) fn submit_shell(&mut self, command: String) -> Vec<Op> {
        let id = TaskId(self.next_task);
        self.next_task += 1;
        self.turn.active = Some(id);
        self.turn.active_shell = true;
        self.transcript.push_shell(id, command.clone());
        self.follow = true;
        vec![Op::SubmitShell { id, command }]
    }

    pub(crate) fn submit_text_with_attachments(
        &mut self,
        text: String,
        attachments: Vec<goat_protocol::InputAttachment>,
    ) -> Vec<Op> {
        let id = TaskId(self.next_task);
        self.next_task += 1;
        self.follow = true;
        self.dirty = true;
        if self.turn.active.is_none() {
            self.turn.active = Some(id);
            self.reset_subagents();
        }
        self.queued
            .push((id, text.clone(), None, attachments.clone()));
        vec![Op::SubmitMessage {
            id,
            text,
            display: None,
            attachments,
        }]
    }

    pub(crate) fn submit_command(&mut self, display: String, prompt: String) -> Vec<Op> {
        let id = TaskId(self.next_task);
        self.next_task += 1;
        self.follow = true;
        self.dirty = true;
        if self.turn.active.is_none() {
            self.turn.active = Some(id);
            self.reset_subagents();
        }
        self.queued
            .push((id, prompt.clone(), Some(display.clone()), Vec::new()));
        vec![Op::SubmitMessage {
            id,
            text: prompt,
            display: Some(display),
            attachments: Vec::new(),
        }]
    }

    pub(crate) fn queued_labels(&self) -> Vec<String> {
        if !matches!(self.main_view, MainView::Live) {
            return Vec::new();
        }
        self.queued
            .iter()
            .filter(|(id, _, _, _)| self.turn.active != Some(*id))
            .map(|(_, text, display, attachments)| {
                display
                    .as_deref()
                    .unwrap_or(text)
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map_or_else(
                        || {
                            attachments
                                .first()
                                .map(|a| format!("[image: {}]", a.label))
                                .unwrap_or_default()
                        },
                        str::to_owned,
                    )
            })
            .collect()
    }

    pub(crate) fn restore_queued_to_composer(&mut self) {
        if self.queued.is_empty() {
            return;
        }
        let restored: Vec<(String, Vec<goat_protocol::InputAttachment>)> = self
            .queued
            .drain(..)
            .map(|(_, text, _, attachments)| (text, attachments))
            .collect();
        let draft = self.composer.text();
        self.composer.clear();
        for (index, (text, attachments)) in restored.into_iter().enumerate() {
            if index > 0 {
                self.composer.insert_str("\n");
            }
            self.composer.insert_str(&text);
            self.composer.push_attachments(attachments);
        }
        if !draft.trim().is_empty() {
            self.composer.insert_str("\n");
            self.composer.insert_str(&draft);
        }
        self.dirty = true;
    }

    pub(crate) fn current_model_supports_images(&self) -> bool {
        let Some(model) = &self.model else {
            return false;
        };
        self.models
            .iter()
            .find(|entry| entry.provider == model.provider && entry.model == model.model)
            .is_some_and(|entry| entry.supports_images)
    }

    pub(crate) fn current_efforts(&self) -> Vec<Effort> {
        let Some(model) = &self.model else {
            return Vec::new();
        };
        self.models
            .iter()
            .find(|entry| entry.provider == model.provider && entry.model == model.model)
            .map(|entry| entry.efforts.clone())
            .unwrap_or_default()
    }

    fn effort_choice_options(&self) -> Vec<RuntimeChoice> {
        self.current_efforts()
            .into_iter()
            .map(|effort| {
                let value = effort.as_str().to_owned();
                RuntimeChoice {
                    label: value.clone(),
                    value,
                    description: None,
                }
            })
            .collect()
    }

    fn model_choice_options(&self) -> Vec<RuntimeChoice> {
        self.models
            .iter()
            .map(|entry| {
                let name = format!("{}/{}", entry.provider, entry.model);
                let description = entry.context_window.map(|window| {
                    let k = window / 1000;
                    if k > 0 {
                        format!("{k}k")
                    } else {
                        format!("{window}")
                    }
                });
                RuntimeChoice {
                    label: name.clone(),
                    value: name,
                    description,
                }
            })
            .collect()
    }

    pub(crate) fn update_command_menu(&mut self) {
        if self.composer.shell() {
            if self.command_menu.upgrade().is_some() || self.file_menu.upgrade().is_some() {
                self.overlay = Overlay::None;
            }
            return;
        }
        if let Some(query) = self.composer.at_query() {
            if let Some(menu) = self.file_menu.upgrade() {
                menu.lock().unwrap().update(&query);
            } else {
                if !self.files_loaded {
                    self.outbox.push(Op::ListFiles {});
                }
                let (screen, handle) = FileMenuScreen::new(FileMenu::new(
                    self.files.clone(),
                    !self.files_loaded,
                    &query,
                ));
                self.file_menu = handle;
                self.overlay = Overlay::Screen(Box::new(screen));
            }
            return;
        }
        if self.file_menu.upgrade().is_some() {
            self.overlay = Overlay::None;
        }
        let text = self.composer.text();
        let trimmed = text.trim_start();
        let effort_options = self.effort_choice_options();
        let model_options = self.model_choice_options();
        let groups = [
            RuntimeChoiceGroup {
                command: "effort",
                parameter: "level",
                options: &effort_options,
                empty_hint: if self.model.is_some() {
                    "this model does not support reasoning effort"
                } else {
                    "select a model first"
                },
            },
            RuntimeChoiceGroup {
                command: "model",
                parameter: "name",
                options: &model_options,
                empty_hint: "no models yet — run /config to connect a provider",
            },
        ];
        let context = CommandMenuContext { choices: &groups };
        if trimmed.starts_with('/')
            && slash_command_name(trimmed).is_none_or(|name| !name.contains('/'))
        {
            if let Some(menu) = self.command_menu.upgrade() {
                menu.lock()
                    .unwrap()
                    .update(&self.commands, trimmed, &context);
            } else {
                let (screen, handle) =
                    CommandMenuScreen::new(CommandMenu::new(&self.commands, trimmed, &context));
                self.command_menu = handle;
                self.overlay = Overlay::Screen(Box::new(screen));
            }
        } else if self.command_menu.upgrade().is_some() {
            self.overlay = Overlay::None;
        }
    }

    pub(crate) fn clamp_scroll(&mut self, viewport_height: u16, content_width: u16) {
        self.viewport_rows = viewport_height;
        let max = self
            .content_height(content_width)
            .saturating_sub(usize::from(viewport_height));
        if self.follow {
            self.scroll = max;
        } else {
            if self.scroll > max {
                self.scroll = max;
            }
            self.follow = self.scroll >= max;
        }
    }

    pub(crate) fn page_rows(&self) -> usize {
        usize::from(self.viewport_rows.saturating_sub(1)).max(1)
    }

    fn wheel_step(&self) -> usize {
        (usize::from(self.viewport_rows) / 4).max(3)
    }

    pub(crate) fn wheel_scroll_allowed(&self) -> bool {
        matches!(self.overlay, Overlay::None | Overlay::Runs(_))
            || self.command_menu.upgrade().is_some()
            || self.file_menu.upgrade().is_some()
    }

    pub(crate) fn overlay_captures_text(&self) -> bool {
        match &self.overlay {
            Overlay::Screen(screen) => screen.captures_text(),
            _ => false,
        }
    }

    pub(crate) fn selection_allowed(&self) -> bool {
        matches!(self.overlay, Overlay::None | Overlay::Runs(_))
    }

    fn screen_to_cache(&self, col: u16, row: u16, clamp: bool) -> Option<(usize, u16)> {
        let area = self.transcript_area;
        let selectable_len = self.active_transcript().selectable_len();
        if area.height == 0 || selectable_len == 0 {
            return None;
        }
        let bottom = (self.scroll + usize::from(area.height))
            .min(selectable_len)
            .saturating_sub(1);
        let line = if row < area.y {
            if !clamp {
                return None;
            }
            self.scroll
        } else {
            let candidate = self.scroll + usize::from(row - area.y);
            if candidate > bottom {
                if !clamp {
                    return None;
                }
                bottom
            } else {
                candidate
            }
        };
        let left = area.x.saturating_add(crate::layout::PAD_X);
        let content_col = if col < left {
            if !clamp && col < area.x {
                return None;
            }
            0
        } else {
            col - left
        };
        Some((line, content_col))
    }

    fn valid_selection(&self) -> Option<crate::select::Selection> {
        self.selection
            .filter(|_| self.active_transcript().version() == self.selection_version)
    }

    fn copy_selection(&mut self) {
        let Some(sel) = self.valid_selection() else {
            return;
        };
        let text = self
            .active_transcript()
            .selected_text(sel.anchor, sel.focus);
        if text.is_empty() {
            return;
        }
        self.pending_copy = Some(text);
        self.toasts.push(crate::toast::Toast::new(
            goat_protocol::NotifyKind::Info,
            "copied".to_owned(),
        ));
        self.dirty = true;
    }

    pub(crate) fn take_pending_copy(&mut self) -> Option<String> {
        self.pending_copy.take()
    }

    pub(crate) fn take_pending_open(&mut self) -> Option<String> {
        self.pending_open.take()
    }

    fn on_left_click(&mut self, col: u16, row: u16) {
        if !self.selection_allowed() {
            return;
        }
        let Some((line, content_col)) = self.screen_to_cache(col, row, false) else {
            return;
        };
        if let Some(url) = self.active_transcript().url_at(line, content_col) {
            self.pending_open = Some(url);
        } else if let Some(img) = self.active_transcript().image_at(line) {
            self.overlay = Overlay::Screen(Box::new(ImageZoomScreen::new(
                Box::new(img),
                self.picker.clone(),
            )));
        }
    }

    fn on_left_down(&mut self, col: u16, row: u16) {
        let on_content = self.screen_to_cache(col, row, false).is_some();
        let Some(pos) = self.screen_to_cache(col, row, true) else {
            self.selection = None;
            self.last_click = None;
            self.dirty = true;
            return;
        };
        self.selection_version = self.active_transcript().version();
        let now = std::time::Instant::now();
        let double = on_content
            && self.last_click.is_some_and(|(t, l, c)| {
                l == pos.0
                    && c.abs_diff(pos.1) <= 1
                    && now.duration_since(t) < std::time::Duration::from_millis(400)
            });
        if double && let Some((lo, hi)) = self.active_transcript().word_bounds_at(pos.0, pos.1) {
            self.selection = Some(crate::select::Selection {
                anchor: (pos.0, lo),
                focus: (pos.0, hi),
                dragging: false,
            });
            self.last_click = None;
            self.dirty = true;
            return;
        }
        self.selection = Some(crate::select::Selection::new(pos));
        self.last_click = if on_content {
            Some((now, pos.0, pos.1))
        } else {
            None
        };
        self.dirty = true;
    }

    fn on_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        use crossterm::event::MouseButton;
        match mouse.kind {
            MouseEventKind::ScrollUp if self.wheel_scroll_allowed() => {
                self.scroll = self.scroll.saturating_sub(self.wheel_step());
                self.follow = false;
                self.dirty = true;
            }
            MouseEventKind::ScrollDown if self.wheel_scroll_allowed() => {
                self.scroll = self.scroll.saturating_add(self.wheel_step());
                self.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) if self.selection_allowed() => {
                self.on_left_down(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(pos) = self.screen_to_cache(mouse.column, mouse.row, true)
                    && let Some(sel) = self.selection.as_mut()
                    && sel.dragging
                {
                    sel.focus = pos;
                    self.dirty = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(sel) = self.selection {
                    if sel.is_empty() {
                        self.selection = None;
                        self.on_left_click(mouse.column, mouse.row);
                    } else if let Some(active) = self.selection.as_mut() {
                        active.dragging = false;
                    }
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }
    pub(crate) fn transcript(&self) -> &Transcript {
        self.active_transcript()
    }
    pub(crate) fn composer(&self) -> &Composer {
        &self.composer
    }
    pub(crate) fn composer_height(&self, available_width: u16) -> u16 {
        self.composer.desired_height(available_width)
    }

    pub(crate) fn elapsed_secs(&self) -> Option<u64> {
        self.turn.task_start.map(|t| t.elapsed().as_secs())
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.turn.active.is_some()
    }
    pub(crate) fn reset_active_state(&mut self) {
        self.turn.active = None;
        self.turn.active_shell = false;
        self.turn.task_start = None;
        self.turn.thinking = false;
        self.turn.retry = None;
        self.turn.compacting = false;
    }
    pub(crate) fn promote_pending_ask(&mut self) {
        if (matches!(self.overlay, Overlay::None) || self.command_menu.upgrade().is_some())
            && let Some(screen) = self.pending.ask.take()
        {
            self.overlay = Overlay::Screen(Box::new(screen));
            self.dirty = true;
        }
    }
    pub(crate) fn cwd(&self) -> &str {
        &self.cwd
    }
    pub(crate) fn workspace_snapshot(&self) -> Option<&goat_worktree::Workspace> {
        self.git_workspace.as_ref()
    }
    fn refresh_git_branch(&mut self) {
        let Some(ws) = self.git_workspace.as_ref() else {
            return;
        };
        let Some(branch) = ws.head_branch() else {
            return;
        };
        if branch == ws.git_branch {
            return;
        }
        if let Some(ws) = self.git_workspace.as_mut() {
            ws.git_branch = branch;
        }
        self.pr = None;
        self.pr_branch = None;
        self.pr_poll = 0;
        self.dirty = true;
    }
    pub(crate) fn forget_pull_request(&mut self) {
        self.pr = None;
        self.pr_branch = None;
        self.pr_poll = 0;
        self.dirty = true;
    }
    pub(crate) fn current_pr(&self) -> Option<&goat_github::PrInfo> {
        let ws = self.git_workspace.as_ref()?;
        if self.pr_branch.as_deref() == Some(ws.git_branch.as_str()) {
            self.pr.as_ref()
        } else {
            None
        }
    }
    fn take_pending_pr_lookup(&mut self) -> Option<PrLookup> {
        if !self.pr_enabled || self.pr_inflight {
            return None;
        }
        let ws = self.git_workspace.as_ref()?;
        if ws.git_branch.is_empty() {
            return None;
        }
        let stale = self.pr_branch.as_deref() != Some(ws.git_branch.as_str());
        if !stale && self.pr_poll > 0 {
            return None;
        }
        self.pr_inflight = true;
        self.pr_poll = PR_POLL_TICKS;
        Some(PrLookup {
            repo_root: ws.repo_root.clone(),
            branch: ws.git_branch.clone(),
        })
    }
    pub(crate) fn quit_armed(&self) -> bool {
        self.quit_arm.is_some()
    }
    pub(crate) fn clear_armed(&self) -> bool {
        self.clear_arm.is_some()
    }

    pub(crate) fn rewind_armed(&self) -> bool {
        self.rewind_arm.is_some()
    }

    pub(crate) fn push_toast(&mut self, kind: NotifyKind, message: String) {
        self.toasts.push(crate::toast::Toast::new(kind, message));
        self.dirty = true;
    }

    fn persist_config(&mut self, cfg: &goat_config::Config) {
        if let Err(err) = cfg.save() {
            tracing::warn!(error = %err, "failed to save config");
            self.push_toast(
                NotifyKind::Error,
                "could not save settings; change may not persist".to_owned(),
            );
        }
    }

    pub(crate) fn clear_ctx_indicator(&mut self) {
        if let Some(model) = &self.model {
            let key = (model.provider.clone(), model.account.clone());
            self.usage.last.remove(&key);
        }
    }
    pub(crate) fn spinner_frame(&self) -> &'static str {
        symbols::SPINNER[self.spinner % symbols::SPINNER.len()]
    }

    pub(crate) fn working_state(&self) -> Option<crate::transcript::Working> {
        if self.turn.active_shell {
            return None;
        }
        if !self.is_busy() {
            return None;
        }
        let grouped_agents = self.transcript.has_running_subagent_group();
        let label = self
            .retry_status()
            .or_else(|| self.compacting_status())
            .or_else(|| {
                if grouped_agents {
                    None
                } else {
                    self.subagent_status()
                }
            });
        if label.is_none() && (self.transcript_has_running_activity() || grouped_agents) {
            return None;
        }
        Some(crate::transcript::Working {
            elapsed: self.elapsed_secs(),
            label,
            thinking: self.turn.thinking,
            tokens: (self.usage.turn_tokens > 0).then_some(self.usage.turn_tokens),
        })
    }

    fn transcript_has_running_activity(&self) -> bool {
        self.transcript.items.iter().any(|item| {
            matches!(
                item,
                crate::transcript::Item::Tool {
                    status: crate::transcript::ToolStatus::Running,
                    ..
                } | crate::transcript::Item::Shell {
                    status: crate::transcript::ShellStatus::Running,
                    ..
                }
            )
        })
    }

    pub(crate) fn take_notification(&mut self) -> Option<crate::notification::Notification> {
        self.notification_pending.take()
    }

    pub(crate) fn queue_notification(&mut self, notification: crate::notification::Notification) {
        self.notification_pending = Some(notification);
    }

    pub(crate) fn compacting_status(&self) -> Option<String> {
        self.turn
            .compacting
            .then(|| format!("compacting context{}", symbols::ui::ELLIPSIS))
    }

    pub(crate) fn retry_status(&self) -> Option<String> {
        let retry = self.turn.retry.as_ref()?;
        let remaining = retry
            .until
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .div_ceil(1000);
        Some(format!(
            "retrying in {remaining}s{sep}attempt {attempt}/{max}{sep}{reason}{sep}response will restart",
            sep = symbols::ui::SEPARATOR,
            attempt = retry.attempt,
            max = retry.max_attempts,
            reason = retry.reason,
        ))
    }

    pub(crate) fn content_height(&self, width: u16) -> usize {
        self.active_transcript().content_height(
            width,
            self.theme,
            &self.highlighter,
            &self.cwd,
            self.working_state().as_ref(),
            &self.queued_labels(),
        )
    }
    pub(crate) fn scroll(&self) -> usize {
        self.scroll
    }
    pub(crate) fn overlay(&self) -> &Overlay {
        &self.overlay
    }
    pub(crate) fn overlay_mut(&mut self) -> &mut Overlay {
        &mut self.overlay
    }
    pub(crate) fn plan_mode(&self) -> bool {
        self.mode.is_plan()
    }
    pub(crate) fn follow(&self) -> bool {
        self.follow
    }
    pub(crate) fn current_model(&self) -> Option<&ModelTarget> {
        self.model.as_ref()
    }

    pub(crate) fn provider_has_multiple_accounts(&self, provider: &str) -> bool {
        self.account_entries
            .iter()
            .find(|e| e.provider == provider)
            .is_some_and(|e| e.accounts.len() > 1)
    }
    pub(crate) fn toasts(&self) -> &[crate::toast::Toast] {
        &self.toasts
    }

    pub(crate) fn reset_subagents(&mut self) {
        self.subagent_runs.retain(|run| run.done.is_none());
        let viewing_dropped = match self.main_view {
            MainView::Subagent(id) => !self.subagent_runs.iter().any(|run| run.id == id),
            _ => false,
        };
        if viewing_dropped {
            self.close_run_selector();
        } else if self.run_selector().is_some() {
            self.sync_run_selector();
        }
    }

    fn set_main_view(&mut self, view: MainView) {
        if self.main_view != view {
            self.selection = None;
            self.last_click = None;
        }
        self.main_view = view;
    }

    pub(crate) fn active_transcript(&self) -> &Transcript {
        match self.main_view {
            MainView::Live => &self.transcript,
            MainView::Subagent(id) => self
                .subagent_runs
                .iter()
                .find(|run| run.id == id)
                .map_or(&self.transcript, |run| &run.transcript),
            MainView::Process(id) => self
                .process_runs
                .iter()
                .find(|run| run.id == id)
                .map_or(&self.transcript, |run| &run.transcript),
        }
    }

    pub(crate) fn run_targets(&self) -> Vec<RunTarget> {
        let mut targets: Vec<RunTarget> = self
            .subagent_runs
            .iter()
            .map(|r| RunTarget::Subagent(r.id))
            .collect();
        targets.extend(self.process_runs.iter().map(|r| RunTarget::Process(r.id)));
        targets
    }

    pub(crate) fn run_row_count(&self) -> usize {
        self.run_targets().len() + 1
    }

    pub(crate) fn move_run_cursor(&mut self, cursor: usize) {
        if cursor < self.run_row_count() {
            self.overlay = Overlay::Runs(cursor);
            self.dirty = true;
        }
    }

    pub(crate) fn open_run(&mut self, cursor: usize) {
        let view = if cursor == 0 {
            Some(MainView::Live)
        } else {
            self.run_targets()
                .get(cursor - 1)
                .map(|target| target.view())
        };
        if let Some(view) = view {
            self.set_main_view(view);
            self.follow = true;
            self.dirty = true;
        }
    }

    fn sync_run_selector(&mut self) {
        let Some(cursor) = self.run_selector() else {
            return;
        };
        let rows = self.run_row_count();
        if self.run_targets().is_empty() {
            self.close_run_selector();
        } else if cursor >= rows {
            self.overlay = Overlay::Runs(rows - 1);
            self.dirty = true;
        }
    }

    pub(crate) fn close_run_selector(&mut self) {
        self.overlay = Overlay::None;
        self.set_main_view(MainView::Live);
        self.follow = true;
        self.dirty = true;
    }

    pub(crate) fn subagent_runs(&self) -> &[SubagentRunView] {
        &self.subagent_runs
    }
    pub(crate) fn process_runs(&self) -> &[ProcessRunView] {
        &self.process_runs
    }
    pub(crate) fn main_view(&self) -> MainView {
        self.main_view
    }

    pub(crate) fn run_selector(&self) -> Option<usize> {
        match self.overlay {
            Overlay::Runs(cursor) => Some(cursor),
            _ => None,
        }
    }
    pub(crate) fn subagent_status(&self) -> Option<String> {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for run in self.subagent_runs.iter().filter(|run| run.done.is_none()) {
            if let Some(entry) = counts
                .iter_mut()
                .find(|(kind, _)| *kind == run.subagent_type)
            {
                entry.1 += 1;
            } else {
                counts.push((run.subagent_type.as_str(), 1));
            }
        }
        let running: usize = counts.iter().map(|(_, n)| n).sum();
        if running == 0 {
            return None;
        }
        let parts: Vec<String> = counts
            .iter()
            .map(|(kind, n)| format!("{n} {kind}"))
            .collect();
        Some(format!("{running} agents · {}", parts.join(", ")))
    }

    pub(crate) fn process_summary(&self) -> Option<String> {
        let running: Vec<&goat_protocol::ProcessInfo> = self
            .processes
            .iter()
            .filter(|p| p.state == goat_protocol::ProcessState::Running)
            .collect();
        if running.is_empty() {
            return None;
        }
        let mut shown: Vec<String> = running
            .iter()
            .take(3)
            .map(|p| {
                let watch = if p.watched { "*" } else { "" };
                let cmd: String = p
                    .command
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("#{}{watch} {cmd}", p.id)
            })
            .collect();
        if running.len() > 3 {
            shown.push(format!("+{}", running.len() - 3));
        }
        Some(shown.join(", "))
    }

    pub(crate) fn current_context_window(&self) -> Option<u32> {
        let model = self.model.as_ref()?;
        self.context_window
            .get(&(model.provider.clone(), model.account.clone()))
            .copied()
    }

    pub(crate) fn ctx_indicator(&self) -> Option<(f32, u64, u32)> {
        let model = self.model.as_ref()?;
        let window = self.current_context_window()?;
        let key = (model.provider.clone(), model.account.clone());
        let usage = self.usage.last.get(&key)?;
        let used = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let pct = (used as f64 / f64::from(window) * 100.0).min(100.0) as f32;
        Some((pct, used, window))
    }

    pub(crate) fn rate_limit_indicator(&self) -> Option<Vec<(String, f32)>> {
        let model = self.model.as_ref()?;
        let key = (model.provider.clone(), model.account.clone());
        let (snapshot, _) = self.usage.rate_limits.get(&key)?;
        (!snapshot.windows.is_empty()).then(|| {
            snapshot
                .windows
                .iter()
                .map(|window| (window.label.clone(), window.used_percent))
                .collect()
        })
    }
}

impl Settings for App {
    fn theme(&self) -> Theme {
        self.theme
    }

    fn set_theme(&mut self, theme: Theme) {
        self.theme = theme.with_base(self.terminal_bg);
        self.transcript.invalidate();
        for run in &mut self.subagent_runs {
            run.transcript.invalidate();
        }
        let mut cfg = goat_config::Config::load();
        cfg.theme = if theme.is_dark() {
            goat_config::ThemeChoice::Dark
        } else {
            goat_config::ThemeChoice::Light
        };
        self.persist_config(&cfg);
    }

    fn mouse_capture(&self) -> bool {
        self.mouse_capture
    }

    fn set_mouse_capture(&mut self, enabled: bool) {
        self.mouse_capture = enabled;
        tui::set_mouse_capture(enabled);
        let mut cfg = goat_config::Config::load();
        cfg.mouse_capture_enabled = enabled;
        self.persist_config(&cfg);
    }

    fn computer_use(&self) -> bool {
        self.computer_use
    }

    fn set_computer_use(&mut self, enabled: bool) {
        self.computer_use = enabled;
        let mut cfg = goat_config::Config::load();
        cfg.computer_use_enabled = enabled;
        self.persist_config(&cfg);
    }

    fn browser(&self) -> bool {
        self.browser
    }

    fn set_browser(&mut self, enabled: bool) {
        self.browser = enabled;
        let mut cfg = goat_config::Config::load();
        cfg.browser_enabled = enabled;
        self.persist_config(&cfg);
    }
}

impl Viewport for App {
    fn scroll(&self) -> usize {
        self.scroll
    }

    fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
        self.dirty = true;
    }

    fn follow(&self) -> bool {
        self.follow
    }

    fn set_follow(&mut self, follow: bool) {
        self.follow = follow;
        self.dirty = true;
    }

    fn page_rows(&self) -> usize {
        App::page_rows(self)
    }

    fn run_cursor(&self) -> Option<usize> {
        self.run_selector()
    }

    fn run_count(&self) -> usize {
        self.run_row_count()
    }

    fn move_run_cursor(&mut self, cursor: usize) {
        App::move_run_cursor(self, cursor);
    }

    fn open_run(&mut self, cursor: usize) {
        App::open_run(self, cursor);
    }

    fn close_run_selector(&mut self) {
        App::close_run_selector(self);
    }
}

impl Session for App {
    fn models(&self) -> &[ModelEntry] {
        &self.models
    }

    fn current_model(&self) -> Option<&ModelTarget> {
        self.model.as_ref()
    }

    fn threads(&self) -> &[goat_protocol::ThreadSummary] {
        &self.threads
    }

    fn usage(&self) -> &UsageState {
        &self.usage
    }

    fn mode(&self) -> goat_protocol::Mode {
        self.mode
    }

    fn accounts(&self) -> &[AccountEntry] {
        &self.account_entries
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            session_id: self.session_id,
            client_id: self.client_id,
            thread_id: self.thread_id,
            daemon: self.daemon.clone(),
            model: self.model.clone(),
            models_loaded: self.models_loaded,
            mode: self.mode,
            plan_path: self.plan_path.clone(),
            cwd: self.cwd.clone(),
            remote: self.remote.clone(),
            workspace: self.git_workspace.clone(),
            pull_request: self.current_pr().cloned(),
            window_count: self.window_count,
            queued_count: self.queued.len(),
            process_count: self.processes.len(),
            skill_count: self.commands.specs().len(),
            transcript_entries: self.transcript.entry_count(),
            mouse_capture: self.mouse_capture,
            computer_use: self.computer_use,
            browser: self.browser,
            dark_theme: self.theme.is_dark(),
            log_path: goat_config::log_dir().map(|dir| format!("{}/goat.log", dir.display())),
            started: self.started,
        }
    }

    fn is_busy(&self) -> bool {
        self.turn.active.is_some()
    }

    fn queued_len(&self) -> usize {
        self.queued.len()
    }

    fn settings(&mut self) -> &mut dyn Settings {
        self
    }

    fn composer(&mut self) -> &mut dyn goat_command::Composer {
        &mut self.composer
    }

    fn viewport(&mut self) -> &mut dyn Viewport {
        self
    }

    fn notify(&mut self, kind: NotifyKind, message: String) {
        self.push_toast(kind, message);
    }

    fn allocate_task(&mut self) -> TaskId {
        let id = TaskId(self.next_task);
        self.next_task += 1;
        id
    }
}

fn slash_command_name(raw: &str) -> Option<&str> {
    let rest = raw.trim().strip_prefix('/')?;
    let name = rest.split_whitespace().next().unwrap_or(rest);
    (!name.is_empty()).then_some(name)
}

pub(crate) fn shorten_home(path: &Path) -> String {
    let display = path.display().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = display.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    display
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Requested,
    Disconnected,
}

pub async fn run(
    ops: Sender<Op>,
    mut events: Receiver<EngineEvent>,
    mut presence: Receiver<usize>,
    theme: Theme,
    origin: Origin,
    initial_ops: Vec<Op>,
) -> color_eyre::Result<ExitReason> {
    let mut app = App::new(theme, &origin);
    let (mut terminal, picker, background) = tui::init(app.mouse_capture)?;
    app.picker = picker.map(std::sync::Arc::new);
    app.set_terminal_bg(background);
    let result = event_loop(
        &mut terminal,
        &ops,
        &mut events,
        &mut presence,
        app,
        initial_ops,
    )
    .await;
    tui::restore();
    let _ = ops.send(Op::Shutdown {}).await;
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    ops: &Sender<Op>,
    events: &mut Receiver<EngineEvent>,
    presence: &mut Receiver<usize>,
    mut app: App,
    initial_ops: Vec<Op>,
) -> color_eyre::Result<ExitReason> {
    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);

    let (attach_tx, mut attach_rx) = tokio::sync::mpsc::channel(8);

    for op in initial_ops {
        if ops.send(op).await.is_err() {
            app.should_quit = true;
        }
    }

    terminal.draw(|frame| view::render(frame, &mut app))?;
    while !app.should_quit {
        let event = tokio::select! {
            maybe = input.next() => match maybe {
                Some(Ok(ev)) => match prepare_input_event(ev, &attach_tx, app.overlay_captures_text()) {
                    Some(event) => event,
                    None => continue,
                },
                Some(Err(_)) | None => break,
            },
            _ = ticker.tick() => AppEvent::Tick,
            maybe = events.recv() => match maybe {
                Some(ev) => AppEvent::Engine(ev),
                None => AppEvent::EngineClosed,
            },
            maybe = attach_rx.recv() => match maybe {
                Some(event) => event,
                None => AppEvent::Tick,
            },
            Some(count) = presence.recv() => AppEvent::Presence(count),
        };

        for op in app.update(event) {
            if ops.send(op).await.is_err() {
                app.should_quit = true;
            }
        }
        while let Ok(pending) = events.try_recv() {
            for op in app.update(AppEvent::Engine(pending)) {
                if ops.send(op).await.is_err() {
                    app.should_quit = true;
                }
            }
        }

        if let Some(notification) = app.take_notification() {
            crate::notification::spawn(notification);
        }
        if let Some(text) = app.take_pending_copy() {
            copy_to_terminal_clipboard(&text);
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(text);
                    }
                })
                .await;
            });
        }
        if let Some(url) = app.take_pending_open() {
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = open::that(url);
                })
                .await;
            });
        }
        if let Some(PrLookup { repo_root, branch }) = app.take_pending_pr_lookup() {
            let tx = attach_tx.clone();
            tokio::spawn(async move {
                let lookup = branch.clone();
                let pr = tokio::task::spawn_blocking(move || {
                    goat_github::pr_for_branch(&repo_root, &lookup)
                })
                .await
                .ok()
                .flatten();
                let _ = tx.send(AppEvent::PrStatus { branch, pr }).await;
            });
        }
        if app.take_dirty() {
            terminal.draw(|frame| view::render(frame, &mut app))?;
        }
    }
    Ok(if app.exit_requested {
        ExitReason::Requested
    } else {
        ExitReason::Disconnected
    })
}

fn copy_to_terminal_clipboard(text: &str) {
    use base64::Engine as _;
    use std::io::Write as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

fn prepare_input_event(
    ev: CtEvent,
    tx: &tokio::sync::mpsc::Sender<AppEvent>,
    overlay_captures_text: bool,
) -> Option<AppEvent> {
    match &ev {
        CtEvent::Paste(text) if !overlay_captures_text => {
            let text = text.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let fallback = !crate::attachment::paste_contains_only_image_paths(&text);
                let result = tokio::task::spawn_blocking({
                    let text = text.clone();
                    move || {
                        crate::attachment::attachments_from_paste(&text)
                            .map_err(|err| err.to_string())
                    }
                })
                .await
                .unwrap_or_else(|err| Err(err.to_string()));
                let _ = tx
                    .send(AppEvent::AttachmentPaste {
                        text,
                        result,
                        fallback,
                    })
                    .await;
            });
            None
        }
        CtEvent::Key(key)
            if !overlay_captures_text
                && key.kind == KeyEventKind::Press
                && crate::keymap::super_char(key) == Some('v') =>
        {
            let tx = tx.clone();
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(|| {
                    crate::attachment::attachment_from_clipboard().map_err(|err| err.to_string())
                })
                .await
                .unwrap_or_else(|err| Err(err.to_string()));
                let _ = tx.send(AppEvent::ClipboardImage(result)).await;
            });
            None
        }
        _ => Some(AppEvent::Input(ev)),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use goat_protocol::{
        AccountChoice, Event as EngineEvent, ModelEntry, ModelTarget, Op, RateLimitSnapshot,
        RateWindow, RewindDraft, RewindPoint, RewindScope, TaskId, Usage,
    };

    use super::{App, AppEvent, Origin, Overlay};
    use crate::theme::Theme;

    fn test_origin() -> Origin {
        Origin::local(
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        )
    }

    #[test]
    fn a_remote_origin_hides_local_git_chrome() {
        let app = App::new(
            Theme::dark(),
            &Origin::remote("/srv/work".to_owned(), "box".to_owned()),
        );
        assert_eq!(app.cwd(), "box:/srv/work");
        assert!(!app.pr_enabled);
        assert!(app.git_workspace.is_none());
    }

    #[test]
    fn paste_passes_through_when_overlay_captures_text() {
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let ev = crossterm::event::Event::Paste("sk-secret".to_owned());
        let out = super::prepare_input_event(ev, &tx, true);
        assert!(
            matches!(out, Some(super::AppEvent::Input(crossterm::event::Event::Paste(t))) if t == "sk-secret"),
            "with a text-capturing overlay, paste must pass through untouched (not be grabbed as an attachment)"
        );
    }

    fn single_entry(provider: &str, model: &str) -> ModelEntry {
        ModelEntry {
            provider: provider.to_owned(),
            model: model.to_owned(),
            accounts: vec![AccountChoice {
                id: "default".to_owned(),
                display: "default".to_owned(),
                target: ModelTarget {
                    provider: provider.to_owned(),
                    model: model.to_owned(),
                    account: "default".to_owned(),
                    effort: None,
                },
            }],
            context_window: None,
            supports_images: true,
            efforts: Vec::new(),
        }
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn shift_tab_toggles_plan_mode() {
        let mut app = App::new(Theme::dark(), &test_origin());
        assert!(!app.plan_mode());
        let ops = app.on_key(press(KeyCode::BackTab, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SetMode { mode }] if mode.is_plan()),
            "shift+tab must ask the engine to enter plan mode"
        );
        assert!(
            app.plan_mode(),
            "the label flips without waiting for the echo"
        );
        let ops = app.on_key(press(KeyCode::BackTab, KeyModifiers::NONE));
        assert!(matches!(ops.as_slice(), [Op::SetMode { mode }] if !mode.is_plan()));
        assert!(!app.plan_mode());
    }

    #[test]
    fn slash_plan_toggles_the_same_way() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/plan");
        let ops = app.submit();
        assert!(matches!(ops.as_slice(), [Op::SetMode { mode }] if mode.is_plan()));
        assert!(app.plan_mode());
    }

    fn proposed(app: &mut App) -> Vec<Op> {
        app.on_engine(EngineEvent::PlanProposed {
            id: TaskId(1),
            call: goat_protocol::ToolCallId(7),
            plan: "# Plan\n\n- [ ] `cargo nextest run` passes\n".to_owned(),
            path: "/plans/1-demo.md".to_owned(),
        })
    }

    #[test]
    fn plan_proposed_opens_the_sheet_and_approve_resolves_it() {
        let mut app = App::new(Theme::dark(), &test_origin());
        proposed(&mut app);
        assert!(matches!(app.overlay, Overlay::Screen(_)));
        let ops = app.on_key(press(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(
            ops.as_slice(),
            [Op::ResolvePlan {
                decision: goat_protocol::PlanDecision::Approve {},
                ..
            }]
        ));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn reject_collects_feedback_before_resolving() {
        let mut app = App::new(Theme::dark(), &test_origin());
        proposed(&mut app);
        let ops = app.on_key(press(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(ops.is_empty(), "asking for changes must not resolve yet");
        for ch in "too big".chars() {
            assert!(
                app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE))
                    .is_empty()
            );
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        match ops.as_slice() {
            [
                Op::ResolvePlan {
                    decision: goat_protocol::PlanDecision::Reject { feedback },
                    ..
                },
            ] => assert_eq!(feedback, "too big"),
            other => panic!("expected a reject with feedback, got {other:?}"),
        }
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn pasting_into_the_reject_field_is_not_grabbed_as_an_attachment() {
        let mut app = App::new(Theme::dark(), &test_origin());
        proposed(&mut app);
        assert!(
            !app.overlay_captures_text(),
            "while reviewing, paste still belongs to the composer"
        );
        app.on_key(press(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(
            app.overlay_captures_text(),
            "while typing changes, paste must reach the sheet"
        );
        app.update(super::AppEvent::Input(crossterm::event::Event::Paste(
            "split step 2".to_owned(),
        )));
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        match ops.as_slice() {
            [
                Op::ResolvePlan {
                    decision: goat_protocol::PlanDecision::Reject { feedback },
                    ..
                },
            ] => assert_eq!(feedback, "split step 2"),
            other => panic!("expected the pasted text as feedback, got {other:?}"),
        }
    }

    #[test]
    fn esc_leaves_the_sheet_without_deciding() {
        let mut app = App::new(Theme::dark(), &test_origin());
        proposed(&mut app);
        let ops = app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(ops.is_empty(), "esc must not send a decision");
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn leaving_plan_mode_closes_a_stale_sheet() {
        let mut app = App::new(Theme::dark(), &test_origin());
        proposed(&mut app);
        app.update(AppEvent::Engine(EngineEvent::ModeChanged {
            mode: goat_protocol::Mode::Normal,
            plan_path: None,
        }));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(!app.plan_mode());
    }

    #[test]
    fn submit_then_interrupt_emit_ops() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("hi");
        let started = app.submit();
        assert!(matches!(started.as_slice(), [Op::SubmitMessage { .. }]));
        let ops = app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(ops.as_slice(), [Op::Interrupt { .. }]));
    }

    fn user_lines(app: &App) -> usize {
        app.transcript
            .items
            .iter()
            .filter(|item| matches!(item, crate::transcript::Item::User(_)))
            .count()
    }

    fn submit_id(ops: &[Op]) -> TaskId {
        match ops {
            [Op::SubmitMessage { id, .. }] => *id,
            _ => panic!("expected a single SubmitMessage op"),
        }
    }

    #[test]
    fn sender_first_message_renders_once_on_echo() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let ops = app.submit_text_with_attachments("hello".to_owned(), Vec::new());
        let id = submit_id(&ops);
        assert_eq!(user_lines(&app), 0, "no optimistic render");
        assert_eq!(app.turn.active, Some(id));

        app.on_engine(EngineEvent::UserMessage {
            id,
            text: "hello".to_owned(),
            display: None,
            attachments: Vec::new(),
        });
        assert_eq!(user_lines(&app), 1);
        assert!(app.queued.is_empty());

        app.on_engine(EngineEvent::TaskStarted { id });
        assert_eq!(user_lines(&app), 1, "TaskStarted adds no user line");
    }

    #[test]
    fn peer_message_renders_from_echo_and_resets() {
        let mut app = App::new(Theme::dark(), &test_origin());
        assert!(app.turn.active.is_none());
        app.on_engine(EngineEvent::UserMessage {
            id: TaskId(42),
            text: "from another window".to_owned(),
            display: None,
            attachments: Vec::new(),
        });
        assert_eq!(user_lines(&app), 1);
        assert!(app.follow);
    }

    #[test]
    fn steering_echo_does_not_reset_agents() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::TaskStarted { id: TaskId(1) });
        app.follow = false;
        app.on_engine(EngineEvent::UserMessage {
            id: TaskId(2),
            text: "mid turn".to_owned(),
            display: None,
            attachments: Vec::new(),
        });
        assert_eq!(user_lines(&app), 1);
        assert!(!app.follow, "mid-turn echo does not force follow");
    }

    #[test]
    fn in_flight_first_message_excluded_from_queued_labels() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let ops = app.submit_text_with_attachments("hello".to_owned(), Vec::new());
        let _ = submit_id(&ops);
        assert!(app.queued_labels().is_empty());
    }

    #[test]
    fn queued_steering_message_shows_label() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::TaskStarted { id: TaskId(100) });
        let _ = app.submit_text_with_attachments("next up".to_owned(), Vec::new());
        assert_eq!(app.queued_labels(), vec!["next up".to_owned()]);
    }

    #[test]
    fn first_message_then_immediate_interrupt_does_not_double_render() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let ops = app.submit_text_with_attachments("hello".to_owned(), Vec::new());
        let id = submit_id(&ops);
        app.on_engine(EngineEvent::UserMessage {
            id,
            text: "hello".to_owned(),
            display: None,
            attachments: Vec::new(),
        });
        app.on_engine(EngineEvent::TaskStarted { id });
        app.on_engine(EngineEvent::TaskDone {
            id,
            interrupted: true,
        });
        assert_eq!(user_lines(&app), 1);
        assert!(app.composer.text().trim().is_empty());
    }

    #[test]
    fn task_done_queues_notification_only_when_unfocused() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::TaskDone {
            id: TaskId(1),
            interrupted: false,
        });
        assert_eq!(app.take_notification(), None);

        app.update(super::AppEvent::Input(crossterm::event::Event::FocusLost));
        app.on_engine(EngineEvent::TaskDone {
            id: TaskId(2),
            interrupted: false,
        });
        assert_eq!(
            app.take_notification(),
            Some(crate::notification::Notification::Completion)
        );

        app.update(super::AppEvent::Input(crossterm::event::Event::FocusGained));
        app.on_engine(EngineEvent::TaskDone {
            id: TaskId(3),
            interrupted: false,
        });
        assert_eq!(app.take_notification(), None);
    }

    #[test]
    fn ask_started_queues_attention_notification_only_when_unfocused() {
        use goat_protocol::{AskQuestion, ToolCallId};

        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::AskStarted {
            id: TaskId(1),
            call: ToolCallId(1),
            questions: vec![AskQuestion {
                question: "continue?".to_owned(),
                options: Vec::new(),
                multiple: false,
            }],
        });
        assert_eq!(app.take_notification(), None);

        app.update(super::AppEvent::Input(crossterm::event::Event::FocusLost));
        app.on_engine(EngineEvent::AskStarted {
            id: TaskId(2),
            call: ToolCallId(2),
            questions: vec![AskQuestion {
                question: "continue?".to_owned(),
                options: Vec::new(),
                multiple: false,
            }],
        });
        assert_eq!(
            app.take_notification(),
            Some(crate::notification::Notification::Attention)
        );
    }

    #[test]
    fn ctrl_c_while_active_arms_quit_not_interrupt() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("hi");
        app.submit();
        let ops = app.on_ctrl_c();
        assert!(
            ops.is_empty(),
            "Ctrl+C during active task must not interrupt"
        );
        assert!(app.quit_armed());
    }

    #[test]
    fn ctrl_c_when_idle_arms_then_quits() {
        let mut app = App::new(Theme::dark(), &test_origin());
        assert!(!app.quit_armed());
        app.on_ctrl_c();
        assert!(app.quit_armed());
        assert!(!app.should_quit);
        app.on_ctrl_c();
        assert!(app.should_quit);
        assert!(app.exit_requested);
    }

    #[test]
    fn bang_on_empty_enters_shell_mode() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::SHIFT));
        assert!(app.composer.shell());
        assert!(app.composer.is_empty());
    }

    #[test]
    fn bang_mid_text_is_literal() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('l'), KeyModifiers::NONE));
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::SHIFT));
        assert!(!app.composer.shell());
        assert_eq!(app.composer.text(), "l!");
    }

    #[test]
    fn backspace_on_empty_exits_shell_mode() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.on_key(press(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(!app.composer.shell());
    }

    #[test]
    fn esc_on_empty_exits_shell_mode() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.composer.shell());
    }

    #[test]
    fn shell_submit_emits_submit_shell() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.composer.insert_str("echo hi");
        let ops = app.submit();
        assert!(
            matches!(ops.as_slice(), [Op::SubmitShell { command, .. }] if command == "echo hi")
        );
        assert!(app.turn.active.is_some());
        assert!(app.turn.active_shell);
        assert!(matches!(
            app.transcript.items.last(),
            Some(crate::transcript::Item::Shell { .. })
        ));
    }

    #[test]
    fn shell_mode_slash_text_is_not_a_command() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.on_key(press(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(matches!(app.overlay, Overlay::None));
        app.composer.insert_str("usr/bin/true");
        let ops = app.submit();
        assert!(
            matches!(ops.as_slice(), [Op::SubmitShell { command, .. }] if command == "/usr/bin/true")
        );
    }

    #[test]
    fn whitespace_shell_submit_keeps_mode() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.composer.insert_str("   ");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(app.composer.shell());
        assert_eq!(app.composer.text(), "   ");
    }

    #[test]
    fn ctrl_c_during_shell_run_interrupts() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.composer.insert_str("sleep 5");
        app.submit();
        let ops = app.on_ctrl_c();
        assert!(matches!(ops.as_slice(), [Op::Interrupt { .. }]));
        assert!(!app.quit_armed());
        assert!(!app.should_quit);
    }

    #[test]
    fn shell_run_suppresses_working_line() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.composer.insert_str("sleep 5");
        app.submit();
        app.on_engine(EngineEvent::TaskStarted { id: TaskId(1) });
        assert!(app.working_state().is_none());
    }

    #[test]
    fn shell_done_completes_cell_and_clears_state() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.composer.insert_str("echo hi");
        let ops = app.submit();
        let [Op::SubmitShell { id, .. }] = ops.as_slice() else {
            panic!("expected SubmitShell");
        };
        app.on_engine(EngineEvent::ShellDone {
            id: *id,
            output: "hi".to_owned(),
        });
        app.on_engine(EngineEvent::TaskDone {
            id: *id,
            interrupted: false,
        });
        assert!(app.turn.active.is_none());
        assert!(!app.turn.active_shell);
        assert!(matches!(
            app.transcript.items.last(),
            Some(crate::transcript::Item::Shell {
                status: crate::transcript::ShellStatus::Done(output),
                ..
            }) if output == "hi"
        ));
    }

    #[test]
    fn shell_history_recall_restores_mode() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.composer.insert_str("echo 1");
        app.submit();
        assert!(!app.composer.shell());
        app.on_engine(EngineEvent::TaskDone {
            id: TaskId(1),
            interrupted: false,
        });
        app.on_key(press(KeyCode::Up, KeyModifiers::NONE));
        assert!(app.composer.shell());
        assert_eq!(app.composer.text(), "echo 1");
    }

    #[test]
    fn shell_submit_while_active_denies() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("hi");
        app.submit();
        app.on_key(press(KeyCode::Char('!'), KeyModifiers::NONE));
        app.composer.insert_str("echo hi");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(
            !app.toasts.is_empty(),
            "denied shell submit must explain itself"
        );
        assert!(app.composer.shell());
        assert_eq!(app.composer.text(), "echo hi");
    }

    #[test]
    fn esc_idle_arms_then_clears() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("hello");
        app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.clear_armed(), "first Esc must arm clear");
        assert!(!app.composer.is_empty(), "composer must not be cleared yet");
        app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.clear_armed(), "second Esc must disarm");
        assert!(app.composer.is_empty(), "second Esc must clear composer");
    }

    #[test]
    fn double_esc_on_empty_composer_requests_rewind_points() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let first = app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(first.is_empty());
        assert!(app.rewind_armed());

        let second = app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(second, vec![Op::ListRewindPoints {}]);
        assert!(!app.rewind_armed());
    }

    #[test]
    fn rewind_picker_selects_code_and_conversation() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let listed = app.dispatch_slash_command("/rewind");
        assert_eq!(listed, vec![Op::ListRewindPoints {}]);
        app.update(AppEvent::Engine(EngineEvent::RewindPointsListed {
            points: vec![RewindPoint {
                checkpoint_id: 7,
                prompt: "change it".into(),
                created_at: 1,
                code_changes: true,
            }],
        }));
        assert!(matches!(app.overlay, Overlay::Screen(_)));

        let choose_point = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(choose_point.is_empty());
        let choose_action = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            choose_action,
            vec![Op::Rewind {
                checkpoint_id: 7,
                scope: RewindScope::CodeAndConversation,
            }]
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn conversation_rewind_restores_prompt_to_composer() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("discard me");

        app.on_engine(EngineEvent::ConversationRewound {
            draft: RewindDraft {
                text: "restored prompt".into(),
                attachments: Vec::new(),
            },
        });

        assert_eq!(app.composer.text(), "restored prompt");
    }

    #[test]
    fn ctrl_c_dubeolsik_arms_then_quits() {
        let mut app = App::new(Theme::dark(), &test_origin());
        assert!(!app.quit_armed());
        app.on_key(press(KeyCode::Char('ㅊ'), KeyModifiers::CONTROL));
        assert!(app.quit_armed());
        assert!(!app.should_quit);
        app.on_key(press(KeyCode::Char('ㅊ'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn plain_dubeolsik_inserts_into_composer() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('ㅊ'), KeyModifiers::NONE));
        assert!(!app.composer.is_empty());
    }

    #[test]
    fn ctrl_other_key_does_not_insert() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_key(press(KeyCode::Char('ㄴ'), KeyModifiers::CONTROL));
        assert!(app.composer.is_empty());
    }

    #[test]
    fn scroll_follow_resets_on_submit() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.follow = false;
        app.composer.insert_str("hello");
        app.submit();
        assert!(app.follow);
    }

    fn filled_app() -> App {
        let mut app = App::new(Theme::dark(), &test_origin());
        for i in 0..30 {
            app.transcript.push_user(format!("message {i}"));
        }
        app.clamp_scroll(10, 80);
        app
    }

    fn mouse(kind: crossterm::event::MouseEventKind) -> super::AppEvent {
        super::AppEvent::Input(crossterm::event::Event::Mouse(
            crossterm::event::MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        ))
    }

    #[test]
    fn clamp_scroll_materializes_bottom_when_following() {
        let app = filled_app();
        assert!(app.follow);
        assert_eq!(app.scroll, app.content_height(80) - 10);
    }

    #[test]
    fn wheel_up_unfollows_then_bottom_refollows() {
        use crossterm::event::MouseEventKind;
        let mut app = filled_app();
        app.update(mouse(MouseEventKind::ScrollUp));
        assert!(!app.follow);
        app.clamp_scroll(10, 80);
        assert!(!app.follow);
        for _ in 0..40 {
            app.update(mouse(MouseEventKind::ScrollDown));
        }
        app.clamp_scroll(10, 80);
        assert!(app.follow);
    }

    #[test]
    fn wheel_ignored_while_picker_overlay_open() {
        use crossterm::event::MouseEventKind;
        let mut app = filled_app();
        app.update(mouse(MouseEventKind::ScrollUp));
        app.clamp_scroll(10, 80);
        let before = app.scroll;
        app.dispatch_slash_command("/model");
        app.update(mouse(MouseEventKind::ScrollUp));
        assert_eq!(app.scroll, before);
    }

    #[test]
    fn home_and_end_jump_transcript_when_composer_empty() {
        let mut app = filled_app();
        app.on_key(press(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.scroll, 0);
        assert!(!app.follow);
        app.clamp_scroll(10, 80);
        assert_eq!(app.scroll, 0);
        app.on_key(press(KeyCode::End, KeyModifiers::NONE));
        app.clamp_scroll(10, 80);
        assert!(app.follow);
        assert_eq!(app.scroll, app.content_height(80) - 10);
    }

    #[test]
    fn page_up_scrolls_by_viewport_and_unfollows() {
        let mut app = filled_app();
        let bottom = app.scroll;
        app.on_key(press(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(!app.follow);
        assert_eq!(app.scroll, bottom - 9);
    }

    #[test]
    fn clear_command_empties_transcript_and_emits_clear() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.transcript.push_user("earlier message");
        app.scroll = 9;
        app.follow = false;
        app.composer.insert_str("/clear");
        let ops = app.submit();
        assert!(matches!(ops.as_slice(), [Op::Clear {}]));
        assert!(app.transcript.items.is_empty());
        assert_eq!(app.scroll, 0);
        assert!(app.follow);
    }

    #[test]
    fn clear_command_rebinds_even_while_active() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.turn.active = Some(TaskId(1));
        app.transcript.push_user("in flight");
        let ops = app.dispatch_slash_command("/clear");
        assert_eq!(ops, vec![Op::Clear {}]);
        assert!(app.transcript.items.is_empty());
        assert!(app.turn.active.is_none());
    }

    #[test]
    fn slash_model_opens_picker_without_op() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/model");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Screen(_)));
    }

    #[test]
    fn picker_esc_closes() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/model");
        app.submit();
        app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn picker_enter_selects_and_emits_op() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![single_entry("openai", "gpt")],
        });
        app.composer.insert_str("/model");
        app.submit();
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(ops.as_slice(), [Op::SelectModel { .. }]));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn picker_filter_then_select() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![
                single_entry("openai", "gpt"),
                single_entry("anthropic", "claude"),
            ],
        });
        app.composer.insert_str("/model");
        app.submit();
        for ch in "claude".chars() {
            app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.provider == "anthropic")
        );
    }

    #[test]
    fn picker_empty_state_keeps_open_on_enter() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/model");
        app.submit();
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Screen(_)));
    }

    #[test]
    fn unknown_slash_command_submits_as_message() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/bogus");
        let ops = app.submit();
        assert!(matches!(ops.as_slice(), [Op::SubmitMessage { text, .. }] if text == "/bogus"));
        assert!(app.turn.active.is_some());
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn absolute_path_starting_with_slash_submits_as_message() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/var/folders/image.png");
        let ops = app.submit();
        assert!(
            matches!(ops.as_slice(), [Op::SubmitMessage { text, .. }] if text == "/var/folders/image.png")
        );
        assert!(app.turn.active.is_some());
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn slash_help_opens_overlay() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/help");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(app.turn.active.is_none());
        assert!(matches!(
            &app.overlay,
            Overlay::Screen(screen) if matches!(screen.placement(), goat_command::Placement::Overlay)
        ));
        assert!(app.transcript.items.is_empty());
    }

    #[test]
    fn skills_changed_registers_invokable_command() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::SkillsChanged {
            skills: vec![goat_protocol::SkillInfo {
                name: "demo".to_owned(),
                description: "a demo".to_owned(),
                command: None,
            }],
        });
        app.composer.insert_str("/demo");
        let ops = app.submit();
        assert!(matches!(ops.as_slice(), [Op::SubmitMessage { .. }]));
        assert!(app.turn.active.is_some());
    }

    #[test]
    fn unknown_skill_command_submits_as_message() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("/demo");
        let ops = app.submit();
        assert!(matches!(ops.as_slice(), [Op::SubmitMessage { text, .. }] if text == "/demo"));
        assert!(app.turn.active.is_some());
        assert!(app.toasts.is_empty());
    }

    fn entry_with_efforts(
        provider: &str,
        model: &str,
        efforts: Vec<goat_protocol::Effort>,
    ) -> ModelEntry {
        let mut entry = single_entry(provider, model);
        entry.efforts = efforts;
        entry
    }

    fn select_model(app: &mut App, provider: &str, model: &str) {
        app.on_engine(EngineEvent::ModelSelected {
            target: ModelTarget {
                provider: provider.to_owned(),
                model: model.to_owned(),
                account: "default".to_owned(),
                effort: None,
            },
        });
    }

    #[test]
    fn effort_without_model_opens_empty_picker() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let ops = app.dispatch_slash_command("/effort");
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Screen(_)));
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn effort_picker_opens_and_selects() {
        use goat_protocol::Effort;
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![entry_with_efforts(
                "openai",
                "gpt",
                vec![Effort::Low, Effort::High],
            )],
        });
        select_model(&mut app, "openai", "gpt");
        let ops = app.dispatch_slash_command("/effort");
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Screen(_)));
        app.on_key(press(KeyCode::Down, KeyModifiers::NONE));
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.effort == Some(Effort::High))
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn effort_arg_sets_supported_level() {
        use goat_protocol::Effort;
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![entry_with_efforts(
                "openai",
                "gpt",
                vec![Effort::Low, Effort::Medium, Effort::High],
            )],
        });
        select_model(&mut app, "openai", "gpt");
        let ops = app.dispatch_slash_command("/effort high");
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.effort == Some(Effort::High))
        );
    }

    #[test]
    fn effort_arg_rejects_unsupported_level() {
        use goat_protocol::Effort;
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![entry_with_efforts("openai", "gpt", vec![Effort::Low])],
        });
        select_model(&mut app, "openai", "gpt");
        let ops = app.dispatch_slash_command("/effort max");
        assert!(ops.is_empty());
        assert!(app.transcript.items.is_empty());
        assert_eq!(app.toasts.len(), 1);
    }

    #[test]
    fn model_arg_selects_unique_match() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![
                single_entry("openai", "gpt"),
                single_entry("anthropic", "claude"),
            ],
        });
        let ops = app.dispatch_slash_command("/model claude");
        assert!(matches!(ops.as_slice(), [Op::SelectModel { target }] if target.model == "claude"));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn effort_menu_typed_choice_runs_without_modal() {
        use goat_protocol::Effort;
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![entry_with_efforts(
                "openai",
                "gpt",
                vec![Effort::Low, Effort::Medium, Effort::High],
            )],
        });
        select_model(&mut app, "openai", "gpt");
        for ch in "/effort h".chars() {
            app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.effort == Some(Effort::High)),
            "expected direct SelectModel, got {ops:?}"
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn model_menu_typed_choice_selects_without_modal() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![
                single_entry("openai", "gpt"),
                single_entry("anthropic", "claude"),
            ],
        });
        for ch in "/model claude".chars() {
            app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.provider == "anthropic" && target.model == "claude"),
            "expected direct SelectModel, got {ops:?}"
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn model_menu_multi_account_opens_light_account_panel() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![multi_account_entry("openai", "gpt", &["work", "personal"])],
        });
        for ch in "/model openai/gpt".chars() {
            app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(ops.is_empty(), "account choice defers selection");
        assert!(
            matches!(app.overlay, Overlay::Screen(_)),
            "expected light account panel, not a heavy picker"
        );
        app.on_key(press(KeyCode::Down, KeyModifiers::NONE));
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.account == "personal"),
            "expected the second account, got {ops:?}"
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    fn multi_account_entry(provider: &str, model: &str, accounts: &[&str]) -> ModelEntry {
        ModelEntry {
            provider: provider.to_owned(),
            model: model.to_owned(),
            accounts: accounts
                .iter()
                .map(|id| AccountChoice {
                    id: (*id).to_owned(),
                    display: (*id).to_owned(),
                    target: ModelTarget {
                        provider: provider.to_owned(),
                        model: model.to_owned(),
                        account: (*id).to_owned(),
                        effort: None,
                    },
                })
                .collect(),
            context_window: None,
            supports_images: true,
            efforts: Vec::new(),
        }
    }

    #[test]
    fn model_menu_slashed_model_id_selects_without_modal() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![single_entry("openrouter", "anthropic/claude")],
        });
        for ch in "/model openrouter/anthropic/claude".chars() {
            app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.model == "anthropic/claude"),
            "expected direct SelectModel, got {ops:?}"
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn resume_requests_list_then_opens_picker() {
        use goat_protocol::ThreadSummary;
        let mut app = App::new(Theme::dark(), &test_origin());
        let ops = app.dispatch_slash_command("/resume");
        assert!(matches!(ops.as_slice(), [Op::ListThreads {}]));
        assert!(matches!(app.overlay, Overlay::Screen(_)));
        let ops = app.update(AppEvent::Engine(EngineEvent::ThreadsListed {
            threads: vec![ThreadSummary {
                id: 7,
                title: "first chat".to_owned(),
                model: "openai/gpt".to_owned(),
                updated_at: 1,
                live: false,
            }],
        }));
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Screen(_)));
    }

    #[test]
    fn resume_index_resolves_to_resume_op() {
        use goat_protocol::ThreadSummary;
        let mut app = App::new(Theme::dark(), &test_origin());
        let ops = app.dispatch_slash_command("/resume 1");
        assert!(matches!(ops.as_slice(), [Op::ListThreads {}]));
        let ops = app.update(AppEvent::Engine(EngineEvent::ThreadsListed {
            threads: vec![ThreadSummary {
                id: 42,
                title: "chat".to_owned(),
                model: "openai/gpt".to_owned(),
                updated_at: 1,
                live: false,
            }],
        }));
        assert!(matches!(ops.as_slice(), [Op::Resume { thread_id: 42 }]));
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn conversation_restored_rebuilds_transcript() {
        use goat_protocol::{ToolCall, ToolCallId, ToolOutcome, TranscriptEntry};
        let mut app = App::new(Theme::dark(), &test_origin());
        app.transcript.push_user("stale");
        app.on_engine(EngineEvent::ConversationRestored {
            target: ModelTarget {
                provider: "anthropic".to_owned(),
                model: "claude".to_owned(),
                account: "default".to_owned(),
                effort: Some(goat_protocol::Effort::High),
            },
            context_tokens: None,
            compaction_threshold: None,
            entries: vec![
                TranscriptEntry::User {
                    text: "hello".to_owned(),
                    attachments: Vec::new(),
                },
                TranscriptEntry::Assistant {
                    text: "hi there".to_owned(),
                },
                TranscriptEntry::Tool {
                    call: ToolCall {
                        id: ToolCallId(1),
                        name: "Read".to_owned(),
                        display: goat_protocol::ToolDisplay::primary("f.rs"),
                    },
                    outcome: ToolOutcome {
                        ok: true,
                        summary: Some("done".to_owned()),
                        body: None,
                        image: None,
                        git: None,
                    },
                },
            ],
        });
        assert_eq!(app.transcript.items.len(), 3);
        assert!(matches!(
            &app.transcript.items[0],
            crate::transcript::Item::User(_)
        ));
        assert!(matches!(
            &app.transcript.items[2],
            crate::transcript::Item::Tool { .. }
        ));
        assert_eq!(
            app.current_model().and_then(|m| m.effort),
            Some(goat_protocol::Effort::High)
        );
    }

    #[test]
    fn agent_events_route_and_drill_in() {
        use goat_protocol::{ToolCall, ToolCallId, ToolOutcome};
        let mut app = App::new(Theme::dark(), &test_origin());
        app.composer.insert_str("go");
        app.submit();
        let top = app.turn.active.unwrap();
        app.on_engine(EngineEvent::UserMessage {
            id: top,
            text: "go".to_owned(),
            display: None,
            attachments: Vec::new(),
        });
        app.on_engine(EngineEvent::ToolStarted {
            id: top,
            call: ToolCall {
                id: ToolCallId(1),
                name: "Subagent".to_owned(),
                display: goat_protocol::ToolDisplay::primary("explore"),
            },
        });
        let child = TaskId(1 << 32);
        app.on_engine(EngineEvent::SubagentStarted {
            id: child,
            parent: top,
            call: ToolCallId(1),
            subagent_type: "explore".to_owned(),
            label: "look into it".to_owned(),
        });
        assert_eq!(app.subagent_runs().len(), 1);
        app.on_engine(EngineEvent::ToolStarted {
            id: child,
            call: ToolCall {
                id: ToolCallId(1),
                name: "Grep".to_owned(),
                display: goat_protocol::ToolDisplay::primary("x"),
            },
        });
        app.on_engine(EngineEvent::ToolDone {
            id: child,
            call: ToolCallId(1),
            outcome: ToolOutcome {
                ok: true,
                summary: None,
                body: None,
                image: None,
                git: None,
            },
        });

        assert_eq!(app.transcript.items.len(), 2);
        assert_eq!(app.subagent_runs[0].transcript.items.len(), 1);
        assert!(app.subagent_status().is_some_and(|s| s.contains("explore")));

        app.on_engine(EngineEvent::SubagentDone {
            id: child,
            ok: true,
        });
        assert_eq!(app.subagent_runs[0].done, Some(true));
        assert!(app.subagent_status().is_none());

        assert_eq!(app.transcript().items.len(), 2);
        app.open_run(1);
        assert!(matches!(app.main_view, super::MainView::Subagent(_)));
        assert_eq!(app.transcript().items.len(), 1);
        app.close_run_selector();
        assert!(matches!(app.main_view, super::MainView::Live));
        assert_eq!(app.transcript().items.len(), 2);
    }

    #[test]
    fn parallel_agent_group_replaces_tool_rows_and_aggregates_metrics() {
        use goat_protocol::{SubagentGroupMember, ToolCall, ToolCallId, ToolOutcome, Usage};

        let mut app = App::new(Theme::dark(), &test_origin());
        let top = TaskId(4);
        app.on_engine(EngineEvent::TaskStarted { id: top });
        app.on_engine(EngineEvent::SubagentGroupStarted {
            id: top,
            group: ToolCallId(1),
            members: vec![
                SubagentGroupMember {
                    call: ToolCallId(1),
                    subagent_type: "explore".to_owned(),
                    label: "map engine".to_owned(),
                    background: false,
                },
                SubagentGroupMember {
                    call: ToolCallId(2),
                    subagent_type: "critic".to_owned(),
                    label: "review UI".to_owned(),
                    background: false,
                },
            ],
        });
        for id in [1, 2] {
            app.on_engine(EngineEvent::ToolStarted {
                id: top,
                call: ToolCall {
                    id: ToolCallId(id),
                    name: "Subagent".to_owned(),
                    display: goat_protocol::ToolDisplay::primary("Agent"),
                },
            });
        }
        assert_eq!(app.transcript.items.len(), 1);
        assert!(app.working_state().is_none());

        let child = TaskId(8);
        app.on_engine(EngineEvent::SubagentStarted {
            id: child,
            parent: top,
            call: ToolCallId(1),
            subagent_type: "explore".to_owned(),
            label: "map engine".to_owned(),
        });
        app.on_engine(EngineEvent::ToolStarted {
            id: child,
            call: ToolCall {
                id: ToolCallId(1),
                name: "Read".to_owned(),
                display: goat_protocol::ToolDisplay::primary("Read(a.rs)"),
            },
        });
        app.on_engine(EngineEvent::Usage {
            id: child,
            provider: "mock".to_owned(),
            account: "default".to_owned(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Usage::default()
            },
            context_window: Some(1000),
            compaction_threshold: Some(900),
        });
        app.on_engine(EngineEvent::ToolDone {
            id: top,
            call: ToolCallId(1),
            outcome: ToolOutcome {
                ok: true,
                summary: None,
                body: None,
                image: None,
                git: None,
            },
        });
        app.on_engine(EngineEvent::ToolDone {
            id: top,
            call: ToolCallId(2),
            outcome: ToolOutcome {
                ok: false,
                summary: Some("failed".to_owned()),
                body: None,
                image: None,
                git: None,
            },
        });

        let crate::transcript::Item::SubagentGroup(group) = &app.transcript.items[0] else {
            panic!("expected agent group")
        };
        assert_eq!(group.members[0].tools, 1);
        assert_eq!(group.members[0].tokens, 15);
        assert!(matches!(
            group.members[0].status,
            crate::transcript::SubagentMemberStatus::Done(ref outcome) if outcome.ok
        ));
        assert!(matches!(
            group.members[1].status,
            crate::transcript::SubagentMemberStatus::Done(ref outcome) if !outcome.ok
        ));
        assert!(group.finished_at.is_some());
        assert_eq!(app.usage.turn_tokens, 0);
        assert!(app.usage.last.is_empty());
    }

    #[test]
    fn error_during_compaction_clears_compacting_status() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::CompactionStarted { id: TaskId(1) });
        assert!(app.compacting_status().is_some());
        app.on_engine(EngineEvent::Error {
            id: Some(TaskId(1)),
            message: "boom".to_owned(),
            hint: None,
        });
        assert!(app.compacting_status().is_none());
        assert!(!app.is_busy());
    }

    #[test]
    fn ask_defers_while_modal_open_then_promotes_on_close() {
        use goat_protocol::{AskQuestion, ToolCallId};
        let mut app = App::new(Theme::dark(), &test_origin());
        app.dispatch_slash_command("/help");
        app.on_engine(EngineEvent::AskStarted {
            id: TaskId(1),
            call: ToolCallId(9),
            questions: vec![AskQuestion {
                question: "ok?".to_owned(),
                options: Vec::new(),
                multiple: false,
            }],
        });
        assert!(matches!(app.overlay, Overlay::Screen(_)));
        assert!(app.pending.ask.is_some());

        app.overlay = Overlay::None;
        app.promote_pending_ask();
        assert!(matches!(app.overlay, Overlay::Screen(_)));
        assert!(app.pending.ask.is_none());
    }

    #[test]
    fn ctx_and_rate_limit_indicators_use_active_model() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.model = Some(ModelTarget {
            provider: "anthropic".to_owned(),
            model: "sonnet".to_owned(),
            account: "default".to_owned(),
            effort: None,
        });
        app.on_engine(EngineEvent::Usage {
            id: TaskId(1),
            provider: "anthropic".to_owned(),
            account: "default".to_owned(),
            usage: Usage {
                input_tokens: 40_000,
                output_tokens: 5_000,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            context_window: Some(128_000),
            compaction_threshold: None,
        });
        app.on_engine(EngineEvent::RateLimits {
            provider: "anthropic".to_owned(),
            account: "default".to_owned(),
            snapshot: RateLimitSnapshot {
                windows: vec![
                    RateWindow {
                        label: "5h".to_owned(),
                        used_percent: 42.0,
                        resets_at: None,
                    },
                    RateWindow {
                        label: "weekly".to_owned(),
                        used_percent: 18.0,
                        resets_at: None,
                    },
                ],
                representative: Some("5h".to_owned()),
            },
            cached_at: 0,
        });

        let (pct, used, window) = app.ctx_indicator().expect("ctx");
        assert_eq!(used, 45_000);
        assert_eq!(window, 128_000);
        assert!((pct - 35.15625).abs() < f32::EPSILON);

        let rates = app.rate_limit_indicator().expect("rates");
        assert_eq!(
            rates,
            vec![("5h".to_owned(), 42.0), ("weekly".to_owned(), 18.0),]
        );
    }

    #[test]
    fn usage_attributes_to_event_model_not_current() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.model = Some(ModelTarget {
            provider: "anthropic".to_owned(),
            model: "sonnet".to_owned(),
            account: "default".to_owned(),
            effort: None,
        });
        app.on_engine(EngineEvent::Usage {
            id: TaskId(1),
            provider: "openai".to_owned(),
            account: "work".to_owned(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            context_window: Some(128_000),
            compaction_threshold: None,
        });
        let openai = app
            .usage
            .total
            .get(&("openai".to_owned(), "work".to_owned()))
            .copied();
        assert_eq!(openai, Some((10, 5)));
        assert!(
            !app.usage
                .total
                .contains_key(&("anthropic".to_owned(), "default".to_owned()))
        );
        assert_eq!(
            app.context_window
                .get(&("openai".to_owned(), "work".to_owned()))
                .copied(),
            Some(128_000)
        );
        assert!(app.current_context_window().is_none());
    }

    #[test]
    fn presence_updates_window_count_and_marks_dirty() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.take_dirty();
        assert_eq!(app.window_count, 1);

        let ops = app.update(super::AppEvent::Presence(3));
        assert!(ops.is_empty());
        assert_eq!(app.window_count, 3);
        assert!(app.take_dirty());
    }

    #[test]
    fn presence_with_same_count_is_not_dirty() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.update(super::AppEvent::Presence(2));
        app.take_dirty();

        let ops = app.update(super::AppEvent::Presence(2));
        assert!(ops.is_empty());
        assert!(!app.take_dirty());
    }

    #[test]
    fn process_list_updates_summary_and_ignores_exited() {
        let mut app = App::new(Theme::dark(), &test_origin());
        assert!(app.process_summary().is_none());
        app.on_engine(EngineEvent::ProcessListChanged {
            processes: vec![
                goat_protocol::ProcessInfo {
                    id: goat_protocol::RunId(1),
                    command: "pnpm dev".to_owned(),
                    state: goat_protocol::ProcessState::Running,
                    watched: false,
                    exit_code: None,
                },
                goat_protocol::ProcessInfo {
                    id: goat_protocol::RunId(2),
                    command: "gh run watch".to_owned(),
                    state: goat_protocol::ProcessState::Exited,
                    watched: true,
                    exit_code: Some(0),
                },
            ],
        });
        let summary = app.process_summary().expect("running process shown");
        assert!(summary.contains("#1"), "got: {summary}");
        assert!(
            !summary.contains("#2"),
            "exited process must not show: {summary}"
        );
    }

    fn process_started(app: &mut App, id: u64, command: &str) {
        app.on_engine(EngineEvent::ProcessStarted {
            process: goat_protocol::RunId(id),
            command: command.to_owned(),
            watched: false,
        });
    }

    #[test]
    fn process_output_is_captured_into_a_process_run() {
        let mut app = App::new(Theme::dark(), &test_origin());
        process_started(&mut app, 1, "pnpm dev");
        assert_eq!(app.process_runs().len(), 1);
        app.on_engine(EngineEvent::ProcessOutput {
            process: goat_protocol::RunId(1),
            chunk: "listening on :3000".to_owned(),
        });
        let item = app.process_runs()[0]
            .transcript
            .items
            .first()
            .expect("process log item");
        let output = match item {
            crate::transcript::Item::Process { output, .. } => output.as_str(),
            _ => panic!("expected a process log item"),
        };
        assert!(output.contains("listening on :3000"), "got: {output}");
    }

    #[test]
    fn output_before_started_creates_run_lazily() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::ProcessOutput {
            process: goat_protocol::RunId(7),
            chunk: "early line".to_owned(),
        });
        assert_eq!(app.process_runs().len(), 1);
        assert_eq!(app.process_runs()[0].id, goat_protocol::RunId(7));
    }

    #[test]
    fn selector_lists_agents_then_processes() {
        let mut app = App::new(Theme::dark(), &test_origin());
        app.on_engine(EngineEvent::SubagentStarted {
            id: TaskId(9),
            parent: TaskId(0),
            call: goat_protocol::ToolCallId(1),
            subagent_type: "explore".to_owned(),
            label: String::new(),
        });
        process_started(&mut app, 1, "pnpm dev");
        let targets = app.run_targets();
        assert_eq!(targets.len(), 2);
        assert!(matches!(targets[0], super::RunTarget::Subagent(_)));
        assert!(matches!(targets[1], super::RunTarget::Process(_)));
    }

    #[test]
    fn selecting_a_process_swaps_the_main_view() {
        let mut app = App::new(Theme::dark(), &test_origin());
        process_started(&mut app, 1, "pnpm dev");
        app.open_run(1);
        assert!(matches!(app.main_view, super::MainView::Process(_)));
        app.close_run_selector();
        assert!(matches!(app.main_view, super::MainView::Live));
    }

    #[test]
    fn reset_agents_keeps_process_runs_and_view() {
        let mut app = App::new(Theme::dark(), &test_origin());
        process_started(&mut app, 1, "pnpm dev");
        app.open_run(1);
        app.reset_subagents();
        assert_eq!(app.process_runs().len(), 1);
        assert!(matches!(app.main_view, super::MainView::Process(_)));
    }

    #[test]
    fn exit_keeps_run_and_marks_exited() {
        let mut app = App::new(Theme::dark(), &test_origin());
        process_started(&mut app, 1, "pnpm dev");
        app.on_engine(EngineEvent::ProcessExited {
            process: goat_protocol::RunId(1),
            code: Some(1),
            reason: goat_protocol::ProcessExitReason::Natural,
        });
        assert_eq!(app.process_runs().len(), 1);
        assert_eq!(
            app.process_runs()[0].state,
            goat_protocol::ProcessState::Exited
        );
        app.on_engine(EngineEvent::ProcessListChanged {
            processes: vec![goat_protocol::ProcessInfo {
                id: goat_protocol::RunId(1),
                command: "pnpm dev".to_owned(),
                state: goat_protocol::ProcessState::Exited,
                watched: false,
                exit_code: Some(1),
            }],
        });
        assert_eq!(app.process_runs().len(), 1);
    }

    #[test]
    fn reconcile_drops_absent_unviewed_run() {
        let mut app = App::new(Theme::dark(), &test_origin());
        process_started(&mut app, 1, "pnpm dev");
        app.on_engine(EngineEvent::ProcessListChanged { processes: vec![] });
        assert!(app.process_runs().is_empty());
    }

    #[test]
    fn reconcile_retains_viewed_run_even_if_absent() {
        let mut app = App::new(Theme::dark(), &test_origin());
        process_started(&mut app, 1, "pnpm dev");
        app.open_run(1);
        app.on_engine(EngineEvent::ProcessListChanged { processes: vec![] });
        assert_eq!(app.process_runs().len(), 1);
        assert!(matches!(app.main_view, super::MainView::Process(_)));
    }

    fn subagent_started(app: &mut App, id: u64, subagent_type: &str) -> TaskId {
        let child = TaskId((1 << 32) + id);
        app.on_engine(EngineEvent::SubagentStarted {
            id: child,
            parent: TaskId(1),
            call: goat_protocol::ToolCallId(id),
            subagent_type: subagent_type.to_owned(),
            label: String::new(),
        });
        child
    }

    #[test]
    fn arrows_move_the_highlight_without_swapping_the_view() {
        let mut app = App::new(Theme::dark(), &test_origin());
        subagent_started(&mut app, 1, "explore");
        process_started(&mut app, 1, "pnpm dev");

        app.move_run_cursor(0);
        assert_eq!(app.run_selector(), Some(0));
        assert!(
            matches!(app.main_view, super::MainView::Live),
            "opening the selector must not commit to a run"
        );

        app.move_run_cursor(2);
        assert_eq!(app.run_selector(), Some(2));
        assert!(
            matches!(app.main_view, super::MainView::Live),
            "browsing must not swap the body under the user"
        );

        app.open_run(2);
        assert!(matches!(app.main_view, super::MainView::Process(_)));
        assert_eq!(
            app.run_selector(),
            Some(2),
            "selecting keeps the list open on the chosen row"
        );
    }

    #[test]
    fn selecting_main_row_restores_the_live_view() {
        let mut app = App::new(Theme::dark(), &test_origin());
        subagent_started(&mut app, 1, "explore");
        app.move_run_cursor(1);
        app.open_run(1);
        assert!(matches!(app.main_view, super::MainView::Subagent(_)));

        app.move_run_cursor(0);
        app.open_run(0);
        assert!(matches!(app.main_view, super::MainView::Live));
        assert_eq!(app.run_selector(), Some(0));
    }

    #[test]
    fn down_past_the_last_row_closes_the_selector() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new(Theme::dark(), &test_origin());
        subagent_started(&mut app, 1, "explore");
        app.move_run_cursor(1);
        assert_eq!(app.run_selector(), Some(1));

        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.run_selector(), None);
        assert!(matches!(app.main_view, super::MainView::Live));
    }

    #[test]
    fn reset_agents_keeps_a_still_running_agent() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let running = subagent_started(&mut app, 1, "explore");
        let finished = subagent_started(&mut app, 2, "general");
        app.on_engine(EngineEvent::SubagentDone {
            id: finished,
            ok: true,
        });

        app.reset_subagents();

        let ids: Vec<TaskId> = app.subagent_runs().iter().map(|run| run.id).collect();
        assert_eq!(
            ids,
            vec![running],
            "a background subagent must survive the next user message"
        );
    }

    #[test]
    fn reset_agents_leaves_the_view_when_the_shown_run_is_dropped() {
        let mut app = App::new(Theme::dark(), &test_origin());
        let finished = subagent_started(&mut app, 1, "explore");
        app.on_engine(EngineEvent::SubagentDone {
            id: finished,
            ok: true,
        });
        app.open_run(1);
        assert!(matches!(app.main_view, super::MainView::Subagent(_)));

        app.reset_subagents();

        assert!(app.subagent_runs().is_empty());
        assert!(matches!(app.main_view, super::MainView::Live));
    }

    #[test]
    fn a_detached_group_member_completes_on_its_report_not_on_the_tool_row() {
        use crate::transcript::SubagentMemberStatus;

        let mut app = App::new(Theme::dark(), &test_origin());
        let top = TaskId(1);
        app.on_engine(EngineEvent::SubagentGroupStarted {
            id: top,
            group: goat_protocol::ToolCallId(1),
            members: vec![goat_protocol::SubagentGroupMember {
                call: goat_protocol::ToolCallId(1),
                subagent_type: "explore".to_owned(),
                label: "auth flow investigation".to_owned(),
                background: true,
            }],
        });

        app.on_engine(EngineEvent::ToolDone {
            id: top,
            call: goat_protocol::ToolCallId(1),
            outcome: goat_protocol::ToolOutcome {
                ok: true,
                summary: None,
                body: None,
                image: None,
                git: None,
            },
        });
        assert!(
            matches!(member_status(&app), SubagentMemberStatus::Pending),
            "detaching is not finishing — the tool row completing must not mark the member done"
        );

        let child = TaskId(1 << 32);
        app.on_engine(EngineEvent::SubagentStarted {
            id: child,
            parent: top,
            call: goat_protocol::ToolCallId(1),
            subagent_type: "explore".to_owned(),
            label: "auth flow investigation".to_owned(),
        });
        assert!(matches!(member_status(&app), SubagentMemberStatus::Running));

        app.on_engine(EngineEvent::SubagentDone {
            id: child,
            ok: true,
        });
        assert!(
            matches!(member_status(&app), SubagentMemberStatus::Done(_)),
            "the report is what finishes a detached member"
        );
    }

    fn member_status(app: &App) -> &crate::transcript::SubagentMemberStatus {
        use crate::transcript::Item;
        app.transcript()
            .items
            .iter()
            .find_map(|item| match item {
                Item::SubagentGroup(group) => Some(&group.members[0].status),
                _ => None,
            })
            .expect("the group is in the transcript")
    }
}

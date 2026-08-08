use goat_protocol::{
    Event as EngineEvent, Op, ProcessExitReason, ProcessInfo, ProcessState, RunId, TaskId,
    TranscriptEntry,
};

use super::{App, MainView, PendingScreen, ProcessRunView};
use crate::{ask::AskPicker, native_screen::AskScreen};

impl App {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn on_engine(&mut self, event: EngineEvent) -> Vec<Op> {
        match event {
            EngineEvent::TaskStarted { id } => {
                self.turn.active = Some(id);
                self.turn.task_start = Some(std::time::Instant::now());
                self.turn.thinking = false;
                self.usage.turn_tokens = 0;
            }
            EngineEvent::ModelListChanged { entries } => {
                self.models = entries;
                self.models_loaded = true;
            }
            EngineEvent::ModelSelected { target } => self.model = Some(target),
            EngineEvent::ModeChanged { mode, plan_path } => {
                self.mode = mode;
                self.plan_path = plan_path;
                self.dirty = true;
            }
            EngineEvent::PlanProposed {
                call, plan, path, ..
            } => {
                if !self.focused {
                    self.queue_notification(crate::notification::Notification::Attention);
                }
                self.overlay = PendingScreen::Screen(Box::new(goat_commands::PlanScreen::new(
                    call, plan, path,
                )));
                self.dirty = true;
            }
            EngineEvent::ThreadsListed { threads } => {
                self.threads = threads;
            }
            EngineEvent::RewindPointsListed { .. } => {
                self.rewind_arm = None;
                self.dirty = true;
            }
            EngineEvent::ConversationRewound { draft } => {
                self.composer.clear();
                self.composer.insert_str(&draft.text);
                self.composer.push_attachments(draft.attachments);
                self.follow = true;
                self.dirty = true;
            }
            EngineEvent::FilesListed { entries } => {
                self.files = entries;
                self.files_loaded = true;
                if let Some(menu) = self.file_menu.upgrade() {
                    let query = self.composer.at_query().unwrap_or_default();
                    menu.lock().unwrap().fill(self.files.clone(), &query);
                }
            }
            EngineEvent::ConversationRestored {
                target,
                entries,
                context_tokens,
                compaction_threshold,
            } => {
                self.transcript.clear();
                self.reset_subagents();
                self.turn = crate::app::TurnStatus::default();
                self.scroll = 0;
                self.follow = true;
                for entry in entries {
                    match entry {
                        TranscriptEntry::User { text, attachments } => {
                            self.transcript
                                .push_user_with_attachments(text, attachments);
                        }
                        TranscriptEntry::Assistant { text } => {
                            self.transcript.commit_text(&text);
                        }
                        TranscriptEntry::Thinking { text } => {
                            self.transcript.push_thinking(text);
                        }
                        TranscriptEntry::Tool { call, outcome } => {
                            let id = call.id;
                            self.transcript.push_tool(call);
                            self.transcript
                                .finish_tool(id, outcome, self.picker.as_deref());
                        }
                        TranscriptEntry::SubagentGroup { group, members } => {
                            self.transcript.push_restored_agent_group(group, members);
                        }
                        TranscriptEntry::Compaction {
                            tokens_before,
                            tokens_after,
                        } => {
                            self.transcript.push_compaction(tokens_before, tokens_after);
                        }
                        TranscriptEntry::Shell { command, output }
                        | TranscriptEntry::Process { command, output } => {
                            let id = TaskId(0);
                            self.transcript.push_shell(id, command);
                            self.transcript.finish_shell(id, output);
                        }
                    }
                }
                self.clear_ctx_indicator();
                self.compaction_threshold = compaction_threshold;
                if let Some(tokens) = context_tokens {
                    let key = (target.provider.clone(), target.account.clone());
                    self.usage.last.insert(
                        key,
                        goat_protocol::Usage {
                            input_tokens: tokens,
                            ..goat_protocol::Usage::default()
                        },
                    );
                }
                self.model = Some(target);
            }
            EngineEvent::ThinkingDelta { id, chunk } => {
                self.turn.thinking = true;
                if let Some(i) = self.subagent_index(id) {
                    self.subagent_runs[i].transcript.push_thinking_delta(&chunk);
                } else {
                    self.transcript.push_thinking_delta(&chunk);
                }
            }
            EngineEvent::LoginProviders { .. } | EngineEvent::LoginStatus { .. } => {}
            EngineEvent::ThreadBound { thread_id } => {
                self.thread_id = Some(thread_id);
            }
            EngineEvent::ProcessListChanged { processes } => {
                self.reconcile_processes(&processes);
                self.processes = processes;
                self.dirty = true;
            }
            EngineEvent::ProcessStarted {
                process,
                command,
                watched: _,
            } => {
                self.ensure_process_run(process, &command);
                self.dirty = true;
            }
            EngineEvent::ProcessOutput { process, chunk } => {
                self.ensure_process_run(process, "");
                if let Some(run) = self.process_runs.iter_mut().find(|r| r.id == process) {
                    run.transcript.append_process(&chunk);
                }
                if self.main_view == MainView::Process(process) {
                    self.dirty = true;
                }
            }
            EngineEvent::ProcessExited {
                process,
                code,
                reason,
            } => {
                if let Some(run) = self.process_runs.iter_mut().find(|r| r.id == process) {
                    run.state = ProcessState::Exited;
                    run.exit_code = code;
                    run.transcript
                        .finish_process(code, &process_exit_marker(code, reason));
                }
                self.dirty = true;
            }
            EngineEvent::ProcessObserved { .. } => {
                self.dirty = true;
            }
            EngineEvent::CompactionStarted { id } => {
                if self.subagent_index(id).is_none() {
                    self.turn.compacting = true;
                }
                self.dirty = true;
            }
            EngineEvent::CompactionDone {
                id,
                ok,
                tokens_before,
                tokens_after,
                usage,
            } => {
                if let Some(i) = self.subagent_index(id) {
                    if ok {
                        let tokens = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
                        let (parent, call) = {
                            let run = &mut self.subagent_runs[i];
                            run.tokens = run.tokens.saturating_add(tokens);
                            run.transcript.push_compaction(tokens_before, tokens_after);
                            (run.parent, run.call)
                        };
                        self.transcript.add_subagent_tokens(parent, call, tokens);
                    }
                } else {
                    self.turn.compacting = false;
                    if ok {
                        self.usage.turn_tokens +=
                            u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
                        self.transcript.push_compaction(tokens_before, tokens_after);
                        if let Some(model) = &self.model {
                            let key = (model.provider.clone(), model.account.clone());
                            let total = self.usage.total.entry(key.clone()).or_default();
                            total.0 += u64::from(usage.input_tokens);
                            total.1 += u64::from(usage.output_tokens);
                            self.usage.last.insert(
                                key,
                                goat_protocol::Usage {
                                    input_tokens: tokens_after,
                                    ..goat_protocol::Usage::default()
                                },
                            );
                        }
                    }
                }
                self.dirty = true;
            }
            EngineEvent::UserMessage {
                id,
                text,
                display,
                attachments,
            } => {
                let sent_by_us = self
                    .queued
                    .iter()
                    .position(|(queued_id, _, _, _)| *queued_id == id)
                    .map(|pos| self.queued.remove(pos))
                    .is_some();
                if !sent_by_us && self.turn.active.is_none() {
                    self.reset_subagents();
                    self.follow = true;
                }
                self.transcript
                    .push_user_with_display(display.unwrap_or_else(|| text.clone()), attachments);
                self.dirty = true;
            }
            EngineEvent::MessageDequeued {
                id,
                text,
                display: _,
                attachments,
            } => {
                if let Some(pos) = self
                    .queued
                    .iter()
                    .position(|(queued_id, _, _, _)| *queued_id == id)
                {
                    self.queued.remove(pos);
                }
                let draft = self.composer.text();
                self.composer.clear();
                self.composer.insert_str(&text);
                self.composer.push_attachments(attachments);
                if !draft.trim().is_empty() {
                    self.composer.insert_str("\n");
                    self.composer.insert_str(&draft);
                }
                self.dirty = true;
            }
            EngineEvent::Retrying {
                id,
                attempt,
                max_attempts,
                delay_ms,
                reason,
                resets_at: _,
            } => {
                self.turn.thinking = false;
                if let Some(i) = self.subagent_index(id) {
                    self.subagent_runs[i].transcript.discard_stream();
                } else {
                    self.transcript.discard_stream();
                    self.turn.retry = Some(super::RetryState {
                        attempt,
                        max_attempts,
                        reason,
                        until: std::time::Instant::now()
                            + std::time::Duration::from_millis(delay_ms),
                    });
                }
                self.dirty = true;
            }
            EngineEvent::AccountsChanged { providers } => {
                self.account_entries = providers;
            }
            EngineEvent::SkillsChanged { skills } => {
                self.commands.set_skills(&skills);
            }
            EngineEvent::TextDelta { id, chunk } => {
                self.turn.thinking = false;
                if self.subagent_index(id).is_none() {
                    self.turn.retry = None;
                }
                if let Some(i) = self.subagent_index(id) {
                    self.subagent_runs[i].transcript.push_delta(&chunk);
                } else {
                    self.transcript.push_delta(&chunk);
                }
            }
            EngineEvent::TextDone { id, text } => {
                if let Some(i) = self.subagent_index(id) {
                    self.subagent_runs[i].transcript.commit_text(&text);
                } else {
                    self.transcript.commit_text(&text);
                }
            }
            EngineEvent::SubagentGroupStarted { id, group, members } => {
                self.transcript.push_subagent_group(id, group, members);
            }
            EngineEvent::ToolStarted { id, call } => {
                self.turn.thinking = false;
                if self.subagent_index(id).is_none() {
                    self.turn.retry = None;
                }
                if let Some(i) = self.subagent_index(id) {
                    let (parent, parent_call) = {
                        let run = &mut self.subagent_runs[i];
                        run.tools = run.tools.saturating_add(1);
                        run.transcript.push_tool(call);
                        (run.parent, run.call)
                    };
                    self.transcript.add_subagent_tool(parent, parent_call);
                } else if !self.transcript.is_subagent_group_call(id, call.id) {
                    self.transcript.push_tool(call);
                }
            }
            EngineEvent::ToolDone { id, call, outcome } => {
                if let Some(i) = self.subagent_index(id) {
                    self.subagent_runs[i].transcript.finish_tool(
                        call,
                        outcome,
                        self.picker.as_deref(),
                    );
                } else if self.transcript.is_subagent_group_call(id, call) {
                    if !self.transcript.detached_group_member(id, call) {
                        self.transcript.finish_subagent(id, call, outcome);
                    }
                } else {
                    let touched = outcome.ok && self.transcript.touches_pull_request(call);
                    self.transcript
                        .finish_tool(call, outcome, self.picker.as_deref());
                    if touched {
                        self.forget_pull_request();
                    }
                }
            }
            EngineEvent::ShellDone { id, output } => {
                self.transcript.finish_shell(id, output);
            }
            EngineEvent::SubagentStarted {
                id,
                parent,
                call,
                subagent_type,
                label,
            } => {
                self.transcript.start_subagent(parent, call);
                self.subagent_runs.push(super::SubagentRunView {
                    id,
                    parent,
                    call,
                    subagent_type,
                    label,
                    transcript: crate::transcript::Transcript::default(),
                    done: None,
                    tools: 0,
                    tokens: 0,
                    started_at: std::time::Instant::now(),
                    finished_at: None,
                });
            }
            EngineEvent::SubagentDone { id, ok } => {
                if let Some(i) = self.subagent_index(id) {
                    self.subagent_runs[i].done = Some(ok);
                    self.subagent_runs[i].finished_at = Some(std::time::Instant::now());
                    self.subagent_runs[i].transcript.complete(!ok);
                    let (parent, call) = (self.subagent_runs[i].parent, self.subagent_runs[i].call);
                    if self.transcript.detached_group_member(parent, call) {
                        self.transcript.finish_subagent(
                            parent,
                            call,
                            goat_protocol::ToolOutcome {
                                ok,
                                summary: None,
                                body: None,
                                image: None,
                                git: None,
                            },
                        );
                    }
                }
            }
            EngineEvent::TaskDone { interrupted, .. } => {
                if !self.focused {
                    self.queue_notification(crate::notification::Notification::Completion);
                }
                self.transcript.flush_thinking();
                self.transcript.complete(interrupted);
                self.reset_active_state();
                if interrupted {
                    self.restore_queued_to_composer();
                }
            }
            EngineEvent::Error { message, hint, .. } => {
                self.transcript.push_error(message, hint);
                self.reset_active_state();
                self.restore_queued_to_composer();
            }
            EngineEvent::Notify { kind, message } => {
                self.toasts.push(crate::toast::Toast::new(kind, message));
                self.dirty = true;
            }
            EngineEvent::AskStarted {
                id,
                call,
                questions,
            } => {
                if !self.focused {
                    self.queue_notification(crate::notification::Notification::Attention);
                }
                let screen = AskScreen::new(AskPicker::new(questions), id, call);
                if matches!(self.overlay, PendingScreen::None)
                    || self.command_menu.upgrade().is_some()
                {
                    self.overlay = PendingScreen::Screen(Box::new(screen));
                } else {
                    self.pending.ask = Some(screen);
                }
                self.dirty = true;
            }
            EngineEvent::AskDismissed { call, .. } => {
                if self
                    .pending
                    .ask
                    .as_ref()
                    .is_some_and(|screen| screen.call() == call)
                {
                    self.pending.ask = None;
                    self.dirty = true;
                }
            }
            EngineEvent::Usage {
                id,
                provider,
                account,
                usage,
                context_window,
                compaction_threshold,
            } => {
                let tokens = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
                if let Some(i) = self.subagent_index(id) {
                    let (parent, call) = {
                        let run = &mut self.subagent_runs[i];
                        run.tokens = run.tokens.saturating_add(tokens);
                        (run.parent, run.call)
                    };
                    self.transcript.add_subagent_tokens(parent, call, tokens);
                } else {
                    self.usage.turn_tokens = self.usage.turn_tokens.saturating_add(tokens);
                    let key = (provider, account);
                    let total = self.usage.total.entry(key.clone()).or_default();
                    total.0 += u64::from(usage.input_tokens);
                    total.1 += u64::from(usage.output_tokens);
                    if let Some(w) = context_window {
                        self.context_window.insert(key.clone(), w);
                    }
                    self.usage.last.insert(key, usage);
                    if compaction_threshold.is_some() {
                        self.compaction_threshold = compaction_threshold;
                    }
                }
                self.dirty = true;
            }
            EngineEvent::RateLimits {
                provider,
                account,
                snapshot,
                cached_at,
            } => {
                self.usage
                    .rate_limits
                    .insert((provider, account), (snapshot, cached_at));
                self.dirty = true;
            }
        }
        Vec::new()
    }

    pub(crate) fn subagent_index(&self, id: TaskId) -> Option<usize> {
        self.subagent_runs.iter().position(|run| run.id == id)
    }

    fn ensure_process_run(&mut self, id: RunId, command: &str) {
        if self.process_runs.iter().any(|run| run.id == id) {
            return;
        }
        let mut transcript = crate::transcript::Transcript::default();
        transcript.push_process(command.to_owned());
        self.process_runs.push(ProcessRunView {
            id,
            command: command.to_owned(),
            state: ProcessState::Running,
            exit_code: None,
            transcript,
        });
    }

    fn reconcile_processes(&mut self, processes: &[ProcessInfo]) {
        for info in processes {
            self.ensure_process_run(info.id, &info.command);
            if let Some(run) = self.process_runs.iter_mut().find(|r| r.id == info.id) {
                if run.command.is_empty() {
                    run.command.clone_from(&info.command);
                }
                run.state = info.state;
                run.exit_code = info.exit_code;
            }
        }
        let viewed = match self.main_view {
            MainView::Process(id) => Some(id),
            _ => None,
        };
        self.process_runs
            .retain(|run| Some(run.id) == viewed || processes.iter().any(|p| p.id == run.id));
        if self.run_selector().is_some() {
            self.sync_run_selector();
        }
    }
}

fn process_exit_marker(code: Option<i32>, reason: ProcessExitReason) -> String {
    match reason {
        ProcessExitReason::Killed => "[killed]".to_owned(),
        ProcessExitReason::Timeout => "[timed out]".to_owned(),
        ProcessExitReason::Shutdown => "[stopped]".to_owned(),
        ProcessExitReason::Natural => match code {
            Some(0) | None => "[exited]".to_owned(),
            Some(code) => format!("[exited: code {code}]"),
        },
    }
}

use goat_protocol::{
    Event as EngineEvent, NotifyKind, Op, ProcessExitReason, ProcessId, ProcessInfo, ProcessState,
    TaskId, TranscriptEntry,
};

use super::{App, MainView, Overlay, ProcessRunView, ResumeIntent};
use crate::{ask::AskPicker, picker::ThreadPicker};

impl App {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn on_engine(&mut self, event: EngineEvent) -> Vec<Op> {
        let mut ops = Vec::new();
        match event {
            EngineEvent::TaskStarted { id } => {
                self.turn.active = Some(id);
                self.turn.task_start = Some(std::time::Instant::now());
                self.turn.thinking = false;
                self.usage.turn_tokens = 0;
            }
            EngineEvent::ModelListChanged { entries } => {
                if let Overlay::Model(picker) = &mut self.overlay {
                    picker.set_entries(entries.clone());
                }
                self.models = entries;
                self.models_loaded = true;
            }
            EngineEvent::ModelSelected { target } => self.model = Some(target),
            EngineEvent::ThreadsListed { threads } => match self.pending.resume.take() {
                Some(ResumeIntent::Picker) => {
                    self.overlay = Overlay::Thread(ThreadPicker::new(threads));
                }
                Some(ResumeIntent::Index(index)) => match threads.get(index) {
                    Some(thread) => ops.push(Op::Resume {
                        thread_id: thread.id,
                    }),
                    None => {
                        self.push_toast(
                            NotifyKind::Error,
                            format!("no conversation #{}", index + 1),
                        );
                    }
                },
                None => {}
            },
            EngineEvent::ConversationRestored {
                target,
                entries,
                context_tokens,
                compaction_threshold,
            } => {
                self.transcript.clear();
                self.reset_agents();
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
                                .finish_tool(id, outcome, self.picker.as_ref());
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
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.push_thinking_delta(&chunk);
                } else {
                    self.transcript.push_thinking_delta(&chunk);
                }
            }
            EngineEvent::LoginProviders { .. } | EngineEvent::ThreadBound { .. } => {}
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
                if self.agent_index(id).is_none() {
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
                if let Some(i) = self.agent_index(id) {
                    if ok {
                        self.agent_runs[i]
                            .transcript
                            .push_compaction(tokens_before, tokens_after);
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
                    self.reset_agents();
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
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.discard_stream();
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
                if let Overlay::Config(config) = &mut self.overlay {
                    config.set_providers(providers.clone());
                }
                self.account_entries = providers;
            }
            EngineEvent::SkillsChanged { skills } => {
                self.commands.set_skills(&skills);
            }
            EngineEvent::LoginStatus {
                message, done, ok, ..
            } => {
                if let Overlay::Config(config) = &mut self.overlay {
                    match (done, ok) {
                        (false, _) => config.set_account_status(message),
                        (true, true) => config.cancel_stage(),
                        (true, false) => config.set_error(message),
                    }
                }
            }
            EngineEvent::TextDelta { id, chunk } => {
                self.turn.thinking = false;
                if self.agent_index(id).is_none() {
                    self.turn.retry = None;
                }
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.push_delta(&chunk);
                } else {
                    self.transcript.push_delta(&chunk);
                }
            }
            EngineEvent::TextDone { id, text } => {
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.commit_text(&text);
                } else {
                    self.transcript.commit_text(&text);
                }
            }
            EngineEvent::ToolStarted { id, call } => {
                self.turn.thinking = false;
                if self.agent_index(id).is_none() {
                    self.turn.retry = None;
                }
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.push_tool(call);
                } else {
                    self.transcript.push_tool(call);
                }
            }
            EngineEvent::ToolDone { id, call, outcome } => {
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i]
                        .transcript
                        .finish_tool(call, outcome, self.picker.as_ref());
                } else {
                    self.transcript
                        .finish_tool(call, outcome, self.picker.as_ref());
                }
            }
            EngineEvent::ShellDone { id, output } => {
                self.transcript.finish_shell(id, output);
            }
            EngineEvent::AgentStarted {
                id,
                agent_type,
                label,
                ..
            } => {
                self.agent_runs.push(super::AgentRunView {
                    id,
                    agent_type,
                    label,
                    transcript: crate::transcript::Transcript::default(),
                    done: None,
                });
            }
            EngineEvent::AgentDone { id, ok } => {
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].done = Some(ok);
                    self.agent_runs[i].transcript.complete(!ok);
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
                call, questions, ..
            } => {
                if !self.focused {
                    self.queue_notification(crate::notification::Notification::Attention);
                }
                let picker = AskPicker::new(questions);
                if matches!(self.overlay, Overlay::None | Overlay::Commands(_)) {
                    self.overlay = Overlay::Ask(picker, call);
                } else {
                    self.pending.ask = Some((picker, call));
                }
                self.dirty = true;
            }
            EngineEvent::AskDismissed { call, .. } => {
                if matches!(&self.overlay, Overlay::Ask(_, c) if *c == call) {
                    self.overlay = Overlay::None;
                    self.dirty = true;
                }
                if matches!(&self.pending.ask, Some((_, c)) if *c == call) {
                    self.pending.ask = None;
                    self.dirty = true;
                }
            }
            EngineEvent::Usage {
                id: _,
                provider,
                account,
                usage,
                context_window,
                compaction_threshold,
            } => {
                self.usage.turn_tokens +=
                    u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
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
        ops
    }

    pub(crate) fn agent_index(&self, id: TaskId) -> Option<usize> {
        self.agent_runs.iter().position(|run| run.id == id)
    }

    fn ensure_process_run(&mut self, id: ProcessId, command: &str) {
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
        if matches!(self.overlay, Overlay::Runs(_)) {
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

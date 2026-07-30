use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use futures::{StreamExt, stream};
use goat_agent_command::{CommandOutput, CommandRegistry};
use goat_agent_config::AgentCard;
use goat_agent_tool::{
    ToolCall, ToolContext, ToolOutput, ToolReadState, ToolRegistry, selector_allows,
    selector_allows_empty_denies, validate_tool_selectors,
};
use goat_bus::{EventBus, EventFilter};
use goat_channel::ChannelHandle;
use goat_model::{Model, canonicalize_provider_id};
use goat_provider::{
    ChunkStream, ContentBlock, Message, MessageRole, Provider, Request, StreamChunk, StreamError,
    ToolChoice, ToolDefinition,
};
use goat_render::{RenderSummary, StreamRenderer};
use goat_skills::SkillIndex;
use goat_store::{
    Direction, HistoryRow, ScheduledTaskStatus, Store, TaskRunStatus, ToolInvocationRecord,
    ToolInvocationStatus,
};
use goat_types::{
    AgentId, Event, IncomingMessage, IntegrationId, IntegrationUpdateKind, MessageId, Surface,
    ThreadId,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug)]
enum ContentPart {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        id: String,
        content: String,
    },
}

#[derive(Clone, Debug)]
struct LlmMessage {
    role: Role,
    content: Vec<ContentPart>,
}

impl LlmMessage {
    fn user_text(s: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::Text(s.into())],
        }
    }
}

#[derive(Clone, Debug)]
struct ToolSpec {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

fn role_to_sdk(role: Role) -> MessageRole {
    match role {
        Role::User | Role::Tool => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
    }
}

fn content_to_sdk(part: &ContentPart) -> ContentBlock {
    match part {
        ContentPart::Text(text) => ContentBlock::Text { text: text.clone() },
        ContentPart::ToolCall {
            id,
            name,
            arguments,
        } => ContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: arguments.clone(),
        },
        ContentPart::ToolResult { id, content } => ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: vec![ContentBlock::Text {
                text: content.clone(),
            }],
            is_error: false,
        },
    }
}

fn message_to_sdk(message: &LlmMessage) -> Message {
    Message {
        role: role_to_sdk(message.role),
        content: message.content.iter().map(content_to_sdk).collect(),
    }
}

fn tool_to_sdk(tool: &ToolSpec) -> ToolDefinition {
    ToolDefinition {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.input_schema.clone(),
    }
}

fn build_request(
    model: &Model,
    system: Option<String>,
    messages: &[LlmMessage],
    tools: &[ToolSpec],
    temperature: Option<f32>,
) -> Request {
    Request {
        model: model.id.clone(),
        messages: messages.iter().map(message_to_sdk).collect(),
        tools: tools.iter().map(tool_to_sdk).collect(),
        effort: None,
        tool_choice: ToolChoice::Auto,
        temperature,
        max_tokens: None,
        system,
    }
}

const RUNTIME_SYSTEM_GUARD: &str = r#"
<goat_runtime_guard>
You are speaking directly to the user through a chat channel.
Return only the final user-facing answer.
Do not reveal or narrate hidden reasoning, prompt analysis, implementation notes, tool-loop state, or conversation bookkeeping.
Do not write phrases such as "we need to respond", "let's craft", "the user asked", "the assistant already", or "now continue the conversation".
When you use tools, wait for tool results and then answer once; do not describe internal tool orchestration unless the user explicitly asks.
When showing command output, preserve line breaks and prefer fenced code blocks.
</goat_runtime_guard>
"#;

const SUMMARY_SYSTEM_PROMPT: &str = r"You maintain a running summary of an ongoing chat conversation so older turns can be dropped from the live context without losing what matters.
Given the previous summary (if any) and the next batch of messages, produce a single updated summary.
Preserve durable facts, decisions, commitments, open questions, and user preferences. Drop small talk and redundant detail.
Write in compact third-person notes. Output only the summary text, no preamble.";

const RECALL_SNIPPET_CHARS: usize = 240;
const MAX_SUMMARY_FOLD_BATCH: usize = 40;
const MAX_SUMMARY_FOLDS_PER_TURN: usize = 4;

const DEFAULT_ACCOUNT: &str = "default";

#[derive(Clone)]
struct ProviderEntry {
    account: String,
    provider: Arc<dyn Provider>,
}

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    entries: Vec<ProviderEntry>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_providers(providers: Vec<Arc<dyn Provider>>) -> Self {
        let mut registry = Self::default();
        for provider in providers {
            registry.insert(provider);
        }
        registry
    }

    pub fn insert(&mut self, provider: Arc<dyn Provider>) {
        self.insert_account(DEFAULT_ACCOUNT, provider);
    }

    pub fn insert_account(&mut self, account: impl Into<String>, provider: Arc<dyn Provider>) {
        self.entries.push(ProviderEntry {
            account: account.into(),
            provider,
        });
    }

    pub fn route(&self, model: &Model) -> Result<Arc<dyn Provider>> {
        let canonical = canonicalize_provider_id(model.provider.0.as_str());
        let account = model.account().unwrap_or(DEFAULT_ACCOUNT);
        let pick = |want: &str| {
            self.entries
                .iter()
                .find(|entry| entry.provider.id().0 == canonical && entry.account == want)
        };
        pick(account)
            .or_else(|| pick(DEFAULT_ACCOUNT))
            .map(|entry| entry.provider.clone())
            .ok_or_else(|| anyhow!("no provider for model {model}"))
    }
}

pub struct BrainDeps {
    pub agent: AgentId,
    pub personality: Arc<AgentCard>,
    pub default_model: Model,
    pub history_window: usize,
    pub tool_selectors: Vec<String>,
    pub providers: Arc<ProviderRegistry>,
    pub tools: Arc<ToolRegistry>,
    pub commands: Arc<CommandRegistry>,
    pub store: Arc<dyn Store>,
    pub memory_engine: Arc<goat_memory::MemoryEngine>,
    pub memory_enabled: bool,
    pub summarize_enabled: bool,
    pub renderer: Arc<dyn StreamRenderer>,
    pub goat_root: PathBuf,
    pub stream_idle_timeout: std::time::Duration,
    pub llm_max_retries: usize,
    pub integration_tools: Vec<String>,
    pub intake_debounce: std::time::Duration,
    pub intake_ceiling: std::time::Duration,
}

pub struct Brain {
    agent: AgentId,
    personality: Arc<AgentCard>,
    default_model: Model,
    history_window: usize,
    tool_selectors: Vec<String>,
    providers: Arc<ProviderRegistry>,
    tools: Arc<ToolRegistry>,
    commands: Arc<CommandRegistry>,
    store: Arc<dyn Store>,
    memory_engine: Arc<goat_memory::MemoryEngine>,
    memory_enabled: bool,
    summarize_enabled: bool,
    renderer: Arc<dyn StreamRenderer>,
    goat_root: PathBuf,
    stream_idle_timeout: std::time::Duration,
    llm_max_retries: usize,
    integration_tools: Vec<String>,
    intake_debounce: std::time::Duration,
    intake_ceiling: std::time::Duration,
}

impl Brain {
    pub fn new(deps: BrainDeps) -> Self {
        Self {
            agent: deps.agent,
            personality: deps.personality,
            default_model: deps.default_model,
            history_window: deps.history_window,
            tool_selectors: deps.tool_selectors,
            providers: deps.providers,
            tools: deps.tools,
            commands: deps.commands,
            store: deps.store,
            memory_engine: deps.memory_engine,
            memory_enabled: deps.memory_enabled,
            summarize_enabled: deps.summarize_enabled,
            renderer: deps.renderer,
            goat_root: deps.goat_root,
            stream_idle_timeout: deps.stream_idle_timeout,
            llm_max_retries: deps.llm_max_retries,
            integration_tools: deps.integration_tools,
            intake_debounce: deps.intake_debounce,
            intake_ceiling: deps.intake_ceiling,
        }
    }

    pub async fn run(
        self: Arc<Self>,
        bus: EventBus,
        channels: Vec<Arc<dyn ChannelHandle>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut sub = bus.subscribe(EventFilter::Persona(self.agent));
        info!(agent = %self.agent, "brain running");

        let mut buffer = IntakeBuffer::new(self.intake_debounce, self.intake_ceiling);
        loop {
            let deadline = buffer.next_deadline();
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                () = wait_intake(deadline) => {
                    for msg in buffer.drain_due(Instant::now()) {
                        if let Err(e) = self.handle_turn(&channels, msg).await {
                            warn!(agent = %self.agent, error = ?e, "turn failed");
                        }
                    }
                }
                event = sub.recv() => {
                    let Some(event) = event else { break };
                    match event {
                        Event::Incoming(msg) => {
                            if !self.should_engage(&msg).await.unwrap_or(false) {
                                continue;
                            }
                            if let Err(e) = self.store.append_incoming(&msg).await {
                                warn!(agent = %self.agent, error = ?e, "append incoming");
                                continue;
                            }
                            let key = (msg.thread.clone(), msg.from.external.clone());
                            if msg.command.is_some() {
                                if let Some(prev) = buffer.take(&key)
                                    && let Err(e) = self.handle_turn(&channels, prev.last).await
                                {
                                    warn!(agent = %self.agent, error = ?e, "turn failed");
                                }
                                if let Err(e) = self.handle_turn(&channels, msg).await {
                                    warn!(agent = %self.agent, error = ?e, "turn failed");
                                }
                            } else {
                                buffer.push(key, msg, Instant::now());
                            }
                        }
                        Event::Schedule {
                            run_id, task_id, ..
                        } => {
                            if let Err(e) = self.handle_schedule(&channels, run_id, task_id).await {
                                warn!(
                                    agent = %self.agent,
                                    run_id,
                                    task_id,
                                    error = ?e,
                                    "schedule failed",
                                );
                            }
                        }
                        Event::IntegrationUpdate {
                            integration,
                            account,
                            kind,
                            external_ref,
                            summary,
                            observation,
                            ..
                        } => {
                            let update = IntegrationTurn {
                                integration,
                                account,
                                kind,
                                external_ref,
                                summary,
                                observation,
                            };
                            if let Err(e) =
                                self.handle_integration_update(&channels, update).await
                            {
                                warn!(
                                    agent = %self.agent,
                                    error = ?e,
                                    "integration update failed",
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    async fn should_engage(&self, msg: &IncomingMessage) -> Result<bool> {
        match engage_decision(msg.surface, msg.addressed, msg.command.is_some()) {
            Engagement::Skip => Ok(false),
            Engagement::NeedsActivity => Ok(self
                .store
                .has_agent_activity(self.agent, &msg.thread)
                .await?),
            Engagement::Engage => Ok(true),
        }
    }

    async fn handle_turn(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        msg: IncomingMessage,
    ) -> Result<()> {
        let handle = channels
            .iter()
            .find(|h| h.id() == msg.thread.channel && h.instance() == msg.thread.instance)
            .cloned()
            .ok_or_else(|| anyhow!("no channel handle for {:?}", msg.thread))?;

        let turn = handle.prepare_turn(&msg).await?;
        let reply_to = turn.reply_to.clone();
        let _typing = turn.typing;

        let (summary, mut messages) = self.load_context(&msg.thread).await?;
        if let Some(call) = msg.command.clone() {
            match self.commands.call(call).await {
                Ok(CommandOutput::Query { content }) => messages.push(LlmMessage {
                    role: Role::User,
                    content: vec![ContentPart::Text(content)],
                }),
                Ok(CommandOutput::Reply { text }) => {
                    let summary = self
                        .renderer
                        .render(
                            handle,
                            msg.thread.clone(),
                            reply_to.clone(),
                            text_stream(self.default_model.clone(), text),
                        )
                        .await?;
                    if !summary.final_text.is_empty() {
                        self.store
                            .append_outgoing_text(
                                self.agent,
                                &msg.thread,
                                &summary.final_text,
                                Some(&msg.id),
                            )
                            .await
                            .context("append outgoing")?;
                    }
                    return Ok(());
                }
                Ok(_) => return Ok(()),
                Err(e) => {
                    warn!(agent = %self.agent, error = ?e, "command failed");
                    messages.push(LlmMessage {
                        role: Role::User,
                        content: vec![ContentPart::Text(format!(
                            "The requested command failed before execution: {e}"
                        ))],
                    });
                }
            }
        }

        let thread_open =
            (msg.surface == Surface::Channel && handle.supports_threads()).then(|| ThreadOpenCtx {
                anchor: msg.id.clone(),
            });

        let (summary, thread) = self
            .complete_with_tools(
                handle,
                TurnRoute {
                    thread: msg.thread.clone(),
                    reply_to,
                    surface: msg.surface,
                    thread_open,
                },
                &mut messages,
                TurnMode::Normal,
                summary,
            )
            .await?;

        if !summary.final_text.is_empty() {
            self.store
                .append_outgoing_text(self.agent, &thread, &summary.final_text, Some(&msg.id))
                .await
                .context("append outgoing")?;
        }

        Ok(())
    }

    async fn build_memory_section(&self, query_text: Option<&str>) -> Option<String> {
        if !self.memory_enabled {
            return None;
        }
        self.build_engine_section(query_text).await
    }

    async fn build_goals_section(&self) -> Option<String> {
        let goals = self.store.active_goals(self.agent).await.ok()?;
        if goals.is_empty() {
            return None;
        }
        let mut out =
            String::from("<active_intentions>\nGoals you are currently working toward:\n");
        for g in goals.iter().take(20) {
            let _ = write!(out, "- [#{} p{}] {}", g.id, g.priority, g.title);
            if let Some(d) = &g.detail
                && !d.trim().is_empty()
            {
                let _ = write!(out, " — {}", d.trim());
            }
            out.push('\n');
        }
        out.push_str("</active_intentions>");
        Some(out)
    }

    async fn build_engine_section(&self, query_text: Option<&str>) -> Option<String> {
        use goat_memory::Scope;
        let scopes = [Scope::Owner, Scope::Self_];
        let mut out = String::new();

        let mut core = String::new();
        for scope in &scopes {
            let files = self
                .memory_engine
                .files()
                .list(scope)
                .await
                .unwrap_or_default();
            for rel in files.into_iter().filter(|r| r.starts_with("core/")) {
                if let Ok(text) = self.memory_engine.files().view(scope, &rel, None).await
                    && !text.trim().is_empty()
                {
                    let _ = writeln!(core, "\n[{}/{}]\n{}", scope.as_key(), rel, text.trim());
                }
            }
        }
        if !core.trim().is_empty() {
            out.push_str("<core_memory>\nAlways-remembered context about the owner and yourself:");
            out.push_str(&core);
            out.push_str("</core_memory>");
        }

        if let Some(query) = query_text.filter(|q| !q.trim().is_empty())
            && let Ok(hits) = self.memory_engine.recall(&scopes, query, 6).await
        {
            let hits: Vec<_> = hits.into_iter().filter(|h| h.kind != "core").collect();
            if !hits.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("<recalled_memory>\nPossibly relevant, with provenance:\n");
                for h in hits {
                    let _ = writeln!(
                        out,
                        "- [{} {}] {}",
                        h.kind,
                        h.source_ref,
                        recall_snippet(&h.text)
                    );
                }
                out.push_str("</recalled_memory>");
            }
        }

        if out.is_empty() { None } else { Some(out) }
    }

    async fn history_messages(&self, conv: &ThreadId) -> Result<Vec<LlmMessage>> {
        let history = self
            .store
            .recent(self.agent, conv, self.history_window)
            .await
            .context("read history")?;
        Ok(rows_to_messages(history))
    }

    async fn load_context(&self, conv: &ThreadId) -> Result<(Option<String>, Vec<LlmMessage>)> {
        if !self.summarize_enabled {
            return Ok((None, self.history_messages(conv).await?));
        }

        let total = self.store.message_count(self.agent, conv).await?;
        let existing = self.store.get_thread_summary(self.agent, conv).await?;
        let mut summary_text = existing.as_ref().map(|s| s.summary.clone());
        let mut summarized = existing.map_or(0, |s| s.summarized_count).min(total);

        let mut folds_done = 0;
        while folds_done < MAX_SUMMARY_FOLDS_PER_TURN
            && total.saturating_sub(summarized) > 2 * self.history_window
        {
            let remaining = total - summarized - self.history_window;
            let fold_count = remaining.min(MAX_SUMMARY_FOLD_BATCH);
            let batch = self
                .store
                .messages_from(self.agent, conv, summarized, fold_count)
                .await?;
            match self.summarize_batch(summary_text.as_deref(), &batch).await {
                Some(updated) => {
                    let new_count = summarized + fold_count;
                    if let Err(e) = self
                        .store
                        .upsert_thread_summary(self.agent, conv, &updated, new_count)
                        .await
                    {
                        warn!(agent = %self.agent, error = ?e, "upsert_thread_summary failed");
                        break;
                    }
                    summary_text = Some(updated);
                    summarized = new_count;
                    folds_done += 1;
                }
                None => break,
            }
        }

        let raw = self
            .store
            .messages_from(
                self.agent,
                conv,
                summarized,
                total.saturating_sub(summarized),
            )
            .await?;
        Ok((summary_text, rows_to_messages(raw)))
    }

    async fn summarize_batch(
        &self,
        previous: Option<&str>,
        batch: &[HistoryRow],
    ) -> Option<String> {
        if batch.is_empty() {
            return None;
        }
        let provider = self.providers.route(&self.default_model).ok()?;
        let transcript = batch
            .iter()
            .map(|h| {
                let who = match h.direction {
                    Direction::In => "user",
                    Direction::Out => "assistant",
                };
                format!("{who}: {}", h.text)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let user = match previous {
            Some(prev) if !prev.trim().is_empty() => format!(
                "PREVIOUS SUMMARY:\n{prev}\n\nNEW MESSAGES:\n{transcript}\n\nUpdated summary:"
            ),
            _ => format!("MESSAGES:\n{transcript}\n\nSummary:"),
        };
        let messages = vec![LlmMessage::user_text(user)];
        let req = build_request(
            &self.default_model,
            Some(SUMMARY_SYSTEM_PROMPT.to_string()),
            &messages,
            &[],
            None,
        );
        let stream = match provider.stream(req).await {
            Ok(s) => s,
            Err(e) => {
                warn!(agent = %self.agent, error = ?e, "summarization request failed");
                return None;
            }
        };
        match fold_turn(stream, self.stream_idle_timeout).await {
            Ok(folded) => {
                let text = folded.text.trim().to_string();
                if text.is_empty() { None } else { Some(text) }
            }
            Err(e) => {
                warn!(agent = %self.agent, error = ?e, "summarization stream failed");
                None
            }
        }
    }

    async fn complete_with_tools(
        &self,
        handle: Arc<dyn ChannelHandle>,
        mut route: TurnRoute,
        messages: &mut Vec<LlmMessage>,
        mode: TurnMode,
        summary: Option<String>,
    ) -> Result<(RenderSummary, ThreadId)> {
        const MAX_TOOL_ROUNDS: usize = 8;

        let provider = self.providers.route(&self.default_model)?;
        let skill_prompt =
            SkillIndex::discover_root(&self.goat_root).system_prompt_block(self.agent);
        let tool_specs: Vec<ToolSpec> = self
            .llm_tool_specs(skill_prompt.is_some(), &mode)
            .into_iter()
            .collect();
        let allowed_tools: HashSet<String> =
            tool_specs.iter().map(|spec| spec.name.clone()).collect();
        let read_state = ToolReadState::default();
        let query_text = messages.iter().rev().find_map(|m| {
            if m.role == Role::User {
                m.content.iter().find_map(|p| match p {
                    ContentPart::Text(t) => Some(t.clone()),
                    _ => None,
                })
            } else {
                None
            }
        });
        let memory_section = {
            let mem = self.build_memory_section(query_text.as_deref()).await;
            let goals = self.build_goals_section().await;
            match (mem, goals) {
                (Some(m), Some(g)) => Some(format!("{m}\n\n{g}")),
                (Some(m), None) => Some(m),
                (None, g) => g,
            }
        };
        let now_iso = chrono::Utc::now().to_rfc3339();
        let base_system = format!(
            "{}\n\n<current_time iso8601=\"{now_iso}\">\nThe current time is {now_iso}. \
             Resolve any user time reference against this clock.\n\
             </current_time>",
            compose_system_prompt(
                &self.personality.system_prompt,
                skill_prompt.as_deref(),
                summary.as_deref(),
                memory_section.as_deref(),
            ),
        );
        let system_prompt = match mode {
            TurnMode::Normal => {
                format!(
                    "{base_system}{}",
                    thread_context_block(route.surface, route.thread_open.is_some())
                )
            }
            TurnMode::Schedule { .. } => format!(
                "{base_system}\n\n<schedule_context>\nYou are running at the \
                 fire moment of a scheduled task. Read the task and act. \
                 If the task is no longer worth doing, reply with exactly: skip\n\
                 </schedule_context>"
            ),
            TurnMode::Integration { .. } => format!(
                "{base_system}\n\n<integration_update_context>\nAn external \
                 service update woke you. Gather context with your tools, store \
                 durable findings in memory, keep the work anchor goal current, \
                 then brief the owner concisely. Do not start the work itself \
                 and take no external write actions beyond the briefing. If \
                 nothing is worth surfacing, reply with exactly: skip\n\
                 </integration_update_context>"
            ),
        };

        for _round in 0..MAX_TOOL_ROUNDS {
            let mut round_specs = tool_specs.clone();
            if route.thread_open.is_some() {
                round_specs.push(open_thread_tool_spec());
            }
            let req = build_request(
                &self.default_model,
                Some(system_prompt.clone()),
                messages,
                &round_specs,
                None,
            );

            let folded = self.stream_with_retry(&provider, req).await?;

            if folded.tool_calls.is_empty() {
                let final_text = sanitize_final_text(folded.text);
                if mode.is_autonomous() && final_text.trim().eq_ignore_ascii_case("skip") {
                    return Ok((
                        RenderSummary {
                            messages_sent: 0,
                            edits: 0,
                            final_text: "skip".into(),
                        },
                        route.thread,
                    ));
                }
                let summary = self
                    .renderer
                    .render(
                        handle,
                        route.thread.clone(),
                        route.reply_to,
                        text_stream(self.default_model.clone(), final_text),
                    )
                    .await?;
                return Ok((summary, route.thread));
            }

            messages.push(assistant_tool_call_message(&folded.tool_calls));

            for call in folded.tool_calls {
                if route.thread_open.is_some() && call.name.as_str() == OPEN_THREAD_TOOL {
                    match parse_open_thread_args(&call.arguments) {
                        None => {
                            route.thread_open = None;
                            messages.push(tool_result_message(
                                call.id,
                                "open_thread needs a non-empty title and seed. Answer inline instead.",
                            ));
                        }
                        Some((title, seed)) => {
                            let anchor = route.thread_open.as_ref().map(|c| c.anchor.clone());
                            match handle
                                .open_thread(&route.thread, anchor.as_ref(), &title)
                                .await
                            {
                                Ok(new_thread) => {
                                    let _ = self
                                        .store
                                        .append_incoming_text(self.agent, &new_thread, &seed)
                                        .await;
                                    route.thread = new_thread;
                                    route.reply_to = None;
                                    route.thread_open = None;
                                    messages.push(tool_result_message(
                                        call.id,
                                        "Opened a new thread; write your answer to the user now.",
                                    ));
                                }
                                Err(e) => {
                                    route.thread_open = None;
                                    messages.push(tool_result_message(
                                        call.id,
                                        format!("Could not open a thread: {e}. Answer inline."),
                                    ));
                                }
                            }
                        }
                    }
                    continue;
                }

                let output = self
                    .execute_tool(&route.thread, &call, read_state.clone(), &allowed_tools)
                    .await;
                messages.push(LlmMessage {
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        id: call.id,
                        content: output.text_for_model(),
                    }],
                });
            }
        }

        if mode.is_autonomous() {
            return Ok((
                RenderSummary {
                    messages_sent: 0,
                    edits: 0,
                    final_text: String::new(),
                },
                route.thread,
            ));
        }
        let text = "I stopped because tool execution exceeded the safety round limit.".to_string();
        let summary = self
            .renderer
            .render(
                handle,
                route.thread.clone(),
                route.reply_to,
                text_stream(self.default_model.clone(), text),
            )
            .await?;
        Ok((summary, route.thread))
    }

    async fn finish_run_logged(&self, run_id: i64, status: TaskRunStatus, note: Option<String>) {
        let label = format!("{status:?}");
        if let Err(e) = self.store.finish_run(run_id, status, note).await {
            tracing::error!(
                run_id,
                agent = %self.agent,
                status = %label,
                error = %e,
                "failed to persist task run completion",
            );
        }
    }

    async fn handle_schedule(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        run_id: i64,
        task_id: i64,
    ) -> Result<()> {
        let task = match self.store.get_scheduled_task(task_id).await? {
            Some(t) if matches!(t.status, ScheduledTaskStatus::Active) => t,
            Some(_) => {
                self.finish_run_logged(
                    run_id,
                    TaskRunStatus::Skipped,
                    Some("task no longer active".into()),
                )
                .await;
                return Ok(());
            }
            None => {
                self.finish_run_logged(
                    run_id,
                    TaskRunStatus::Failed,
                    Some("task row missing".into()),
                )
                .await;
                return Ok(());
            }
        };

        let conv = task.origin_conv.clone();
        let Some(handle) = channels
            .iter()
            .find(|h| h.id() == conv.channel && h.instance() == conv.instance)
            .cloned()
        else {
            let available: Vec<String> = channels
                .iter()
                .map(|h| format!("{}:{}", h.id().as_str(), h.instance()))
                .collect();
            warn!(
                run_id,
                agent = %self.agent,
                want = %format!("{}:{}", conv.channel.as_str(), conv.instance),
                have = ?available,
                "no channel handle for origin_conv; marking failed"
            );
            self.finish_run_logged(
                run_id,
                TaskRunStatus::Failed,
                Some("no channel handle for origin_conv".into()),
            )
            .await;
            return Ok(());
        };

        let mut messages = vec![LlmMessage {
            role: Role::User,
            content: vec![ContentPart::Text(task.task.clone())],
        }];

        let (summary, thread) = match self
            .complete_with_tools(
                handle,
                TurnRoute {
                    thread: conv.clone(),
                    reply_to: None,
                    surface: surface_of_external(&conv.external),
                    thread_open: None,
                },
                &mut messages,
                TurnMode::Schedule {
                    tools: task.tools.clone(),
                },
                None,
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                self.finish_run_logged(
                    run_id,
                    TaskRunStatus::Failed,
                    Some(format!("schedule run errored: {e}")),
                )
                .await;
                return Err(e);
            }
        };

        let trimmed = summary.final_text.trim();
        if trimmed.eq_ignore_ascii_case("skip") {
            self.finish_run_logged(
                run_id,
                TaskRunStatus::Skipped,
                Some("model declined".into()),
            )
            .await;
            return Ok(());
        }
        if trimmed.is_empty() {
            warn!(
                run_id,
                task_id,
                agent = %self.agent,
                "schedule produced empty response; marking failed",
            );
            self.finish_run_logged(
                run_id,
                TaskRunStatus::Failed,
                Some("empty response from model".into()),
            )
            .await;
            return Ok(());
        }

        self.store
            .append_outgoing_text(self.agent, &thread, &summary.final_text, None)
            .await
            .context("append outgoing text for schedule")?;

        let truncated = truncate_for_summary(&summary.final_text);
        self.finish_run_logged(run_id, TaskRunStatus::Done, Some(truncated))
            .await;
        Ok(())
    }

    async fn handle_integration_update(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        update: IntegrationTurn,
    ) -> Result<()> {
        let resolved = match self.store.latest_thread(self.agent).await? {
            Some(thread) => channels
                .iter()
                .find(|h| h.id() == thread.channel && h.instance() == thread.instance)
                .cloned()
                .map(|handle| (thread, handle)),
            None => None,
        };
        let Some((thread, handle)) = resolved else {
            warn!(
                agent = %self.agent,
                integration = %update.integration,
                external_ref = %update.external_ref,
                "no channel handle for integration update; dropping briefing",
            );
            return Ok(());
        };

        let mut messages = vec![LlmMessage {
            role: Role::User,
            content: vec![ContentPart::Text(integration_prompt(&update))],
        }];

        let mut tools = self.integration_tools.clone();
        tools.extend(
            ["memory_search", "fact", "observation"]
                .iter()
                .map(std::string::ToString::to_string),
        );

        let (summary, thread) = self
            .complete_with_tools(
                handle,
                TurnRoute {
                    thread: thread.clone(),
                    reply_to: None,
                    surface: surface_of_external(&thread.external),
                    thread_open: None,
                },
                &mut messages,
                TurnMode::Integration { tools },
                None,
            )
            .await?;

        let trimmed = summary.final_text.trim();
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("skip") {
            self.store
                .append_outgoing_text(self.agent, &thread, &summary.final_text, None)
                .await
                .context("append outgoing text for integration update")?;
        }
        Ok(())
    }

    fn llm_tool_specs(&self, has_skills: bool, mode: &TurnMode) -> Vec<ToolSpec> {
        self.tools
            .default_specs()
            .into_iter()
            .filter(|spec| selector_allows(spec.name.as_str(), &self.tool_selectors))
            .filter(|spec| has_skills || spec.name.as_str() != "skill")
            .filter(|spec| match mode {
                TurnMode::Normal => true,
                TurnMode::Schedule { tools } | TurnMode::Integration { tools } => {
                    !is_schedule_tool(spec.name.as_str())
                        && selector_allows_empty_denies(spec.name.as_str(), tools)
                }
            })
            .map(|spec| ToolSpec {
                name: spec.name.as_str().to_string(),
                description: spec.description.unwrap_or_default(),
                input_schema: spec.input_schema,
            })
            .collect()
    }

    async fn execute_tool(
        &self,
        conv: &ThreadId,
        call: &ModelToolCall,
        read_state: ToolReadState,
        allowed_tools: &HashSet<String>,
    ) -> ToolOutput {
        let started_at = chrono::Utc::now();
        let name = match goat_agent_tool::ToolName::new(call.name.clone()) {
            Ok(name) => name,
            Err(e) => {
                let output = ToolOutput::error(format!("invalid tool requested by model: {e}"));
                self.audit_tool_call(conv, call, call.name.clone(), &output, started_at)
                    .await;
                return output;
            }
        };
        if !allowed_tools.contains(name.as_str()) {
            let output = ToolOutput::error(format!("tool not allowed for this turn: {name}"));
            self.audit_tool_call(conv, call, name.to_string(), &output, started_at)
                .await;
            return output;
        }
        if is_schedule_create_tool(name.as_str())
            && let Err(e) = validate_scheduled_tool_selectors(&call.arguments, allowed_tools)
        {
            let output = ToolOutput::error(e);
            self.audit_tool_call(conv, call, name.to_string(), &output, started_at)
                .await;
            return output;
        }
        let ctx = ToolContext {
            agent: self.agent,
            thread: conv.clone(),
            goat_root: self.goat_root.clone(),
            read_state,
        };
        let tool_call = ToolCall {
            call_id: call.id.clone(),
            name: name.clone(),
            arguments: call.arguments.clone(),
        };
        let resolved_name = name.to_string();
        let output = self.tools.call(ctx, tool_call).await;
        self.audit_tool_call(conv, call, resolved_name, &output, started_at)
            .await;
        output
    }

    async fn audit_tool_call(
        &self,
        conv: &ThreadId,
        call: &ModelToolCall,
        resolved_name: String,
        output: &ToolOutput,
        started_at: chrono::DateTime<chrono::Utc>,
    ) {
        let finished_at = chrono::Utc::now();
        let status = if output.is_error {
            ToolInvocationStatus::Error
        } else {
            ToolInvocationStatus::Ok
        };
        let output_text = output.text_for_model();
        let record = ToolInvocationRecord {
            agent: self.agent,
            thread: conv.clone(),
            call_id: call.id.clone(),
            tool_name: resolved_name,
            args_json: call.arguments.clone(),
            status,
            output_preview: Some(preview(&output_text, 2048)),
            error: output.is_error.then(|| output_text.clone()),
            started_at,
            finished_at,
        };
        if let Err(e) = self.store.append_tool_invocation(record).await {
            warn!(error = ?e, tool = %call.name, "failed to audit tool invocation");
        }
    }
}

#[derive(Debug)]
struct ModelToolCall {
    id: String,
    name: String,
    arguments: serde_json::Value,
}

struct FoldedTurn {
    text: String,
    tool_calls: Vec<ModelToolCall>,
}

fn assistant_tool_call_message(calls: &[ModelToolCall]) -> LlmMessage {
    LlmMessage {
        role: Role::Assistant,
        content: calls
            .iter()
            .map(|call| ContentPart::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect(),
    }
}

struct TurnRoute {
    thread: ThreadId,
    reply_to: Option<MessageId>,
    surface: Surface,
    thread_open: Option<ThreadOpenCtx>,
}

#[derive(Clone, Debug)]
struct ThreadOpenCtx {
    anchor: MessageId,
}

struct Pending {
    last: IncomingMessage,
    first_seen: Instant,
    deadline: Instant,
}

struct IntakeBuffer {
    pending: HashMap<(ThreadId, String), Pending>,
    debounce: std::time::Duration,
    ceiling: std::time::Duration,
}

impl IntakeBuffer {
    fn new(debounce: std::time::Duration, ceiling: std::time::Duration) -> Self {
        Self {
            pending: HashMap::new(),
            debounce,
            ceiling,
        }
    }

    fn push(&mut self, key: (ThreadId, String), msg: IncomingMessage, now: Instant) {
        if let Some(existing) = self.pending.get_mut(&key) {
            existing.last = msg;
            existing.deadline = (now + self.debounce).min(existing.first_seen + self.ceiling);
        } else {
            let deadline = (now + self.debounce).min(now + self.ceiling);
            self.pending.insert(
                key,
                Pending {
                    last: msg,
                    first_seen: now,
                    deadline,
                },
            );
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|p| p.deadline).min()
    }

    fn drain_due(&mut self, now: Instant) -> Vec<IncomingMessage> {
        let due: Vec<(ThreadId, String)> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(k, _)| k.clone())
            .collect();
        due.into_iter()
            .filter_map(|k| self.pending.remove(&k).map(|p| p.last))
            .collect()
    }

    fn take(&mut self, key: &(ThreadId, String)) -> Option<Pending> {
        self.pending.remove(key)
    }
}

async fn wait_intake(deadline: Option<Instant>) {
    match deadline {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending::<()>().await,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Engagement {
    Engage,
    NeedsActivity,
    Skip,
}

fn engage_decision(surface: Surface, addressed: bool, has_command: bool) -> Engagement {
    if has_command {
        return Engagement::Engage;
    }
    match surface {
        Surface::Dm => Engagement::Engage,
        Surface::Channel => {
            if addressed {
                Engagement::Engage
            } else {
                Engagement::Skip
            }
        }
        Surface::Thread => {
            if addressed {
                Engagement::Engage
            } else {
                Engagement::NeedsActivity
            }
        }
    }
}

const OPEN_THREAD_TOOL: &str = "open_thread";

fn open_thread_tool_spec() -> ToolSpec {
    ToolSpec {
        name: OPEN_THREAD_TOOL.to_string(),
        description: "Open a new dedicated thread that branches off the current channel message, \
             for a distinct multi-turn task. Provide a short title and the first user-facing \
             message (seed) to post into the new thread. Prefer this over answering inline when \
             the task deserves its own focused space."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short title for the new thread."
                },
                "seed": {
                    "type": "string",
                    "description": "First user-facing message to post into the new thread."
                }
            },
            "required": ["title", "seed"]
        }),
    }
}

fn parse_open_thread_args(args: &serde_json::Value) -> Option<(String, String)> {
    let title = args
        .get("title")
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    let seed = args
        .get("seed")
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    if seed.is_empty() {
        return None;
    }
    Some((title, seed))
}

fn tool_result_message(id: String, content: impl Into<String>) -> LlmMessage {
    LlmMessage {
        role: Role::Tool,
        content: vec![ContentPart::ToolResult {
            id,
            content: content.into(),
        }],
    }
}

fn surface_of_external(external: &str) -> Surface {
    if external.starts_with("dm:") {
        Surface::Dm
    } else {
        Surface::Channel
    }
}

fn thread_context_block(surface: Surface, offers_thread: bool) -> String {
    let location = match surface {
        Surface::Dm => "a direct message",
        Surface::Channel => "a shared channel",
        Surface::Thread => "a thread",
    };
    let mut out = format!("\n\n<thread_context>\nYou are replying in {location}.");
    if offers_thread {
        out.push_str(
            " Open a new thread when starting a distinct multi-turn task; \
             keep casual or quick replies inline.",
        );
    }
    out.push_str("\n</thread_context>");
    out
}

fn is_transient_stream_error(e: &StreamError) -> bool {
    matches!(
        e,
        StreamError::Transport { .. }
            | StreamError::Overloaded { .. }
            | StreamError::RateLimited { .. }
            | StreamError::Other { .. }
    )
}

impl Brain {
    async fn stream_with_retry(
        &self,
        provider: &Arc<dyn Provider>,
        req: Request,
    ) -> Result<FoldedTurn> {
        let mut last_rate_limit_secs: Option<u64> = None;

        for attempt in 0usize..=self.llm_max_retries {
            if attempt > 0 {
                let delay = match last_rate_limit_secs.take() {
                    Some(secs) => std::time::Duration::from_secs(secs),
                    None => std::time::Duration::from_millis(500u64 << (attempt - 1).min(4)),
                };
                warn!(
                    agent = %self.agent,
                    attempt,
                    delay_ms = delay.as_millis(),
                    "retrying transient LLM error",
                );
                tokio::time::sleep(delay).await;
            }

            let outcome = match provider.stream(req.clone()).await {
                Err(e) => Err(e),
                Ok(stream) => fold_turn(stream, self.stream_idle_timeout).await,
            };

            match outcome {
                Ok(folded) => return Ok(folded),
                Err(e) => {
                    let is_last = attempt == self.llm_max_retries;
                    if !is_transient_stream_error(&e) || is_last {
                        return Err(anyhow::anyhow!("{e}"));
                    }
                    if let StreamError::RateLimited { retry_after, .. } = &e {
                        last_rate_limit_secs = retry_after.map(|d| d.as_secs());
                    }
                    warn!(
                        agent = %self.agent,
                        error = ?e,
                        attempt,
                        "LLM stream error; will retry",
                    );
                }
            }
        }

        unreachable!()
    }
}

async fn fold_turn(
    mut stream: ChunkStream,
    idle_timeout: std::time::Duration,
) -> Result<FoldedTurn, StreamError> {
    let mut text = String::new();
    let mut calls: Vec<ModelToolCall> = Vec::new();

    loop {
        match tokio::time::timeout(idle_timeout, stream.next()).await {
            Err(_elapsed) => return Err(StreamError::transport("LLM stream stalled")),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(e),
            Ok(Some(Ok(chunk))) => match chunk {
                StreamChunk::TextDelta { text: delta } => text.push_str(&delta),
                StreamChunk::ToolCall { id, name, input } => {
                    let id = if id.is_empty() {
                        format!("call_{}", calls.len())
                    } else {
                        id
                    };
                    let arguments = if input.trim().is_empty() {
                        serde_json::Value::Object(serde_json::Map::new())
                    } else {
                        serde_json::from_str(&input).unwrap_or_else(|e| {
                            serde_json::json!({"_invalid_json": input, "_error": e.to_string()})
                        })
                    };
                    calls.push(ModelToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            },
        }
    }

    Ok(FoldedTurn {
        text,
        tool_calls: calls,
    })
}

fn text_stream(_model: Model, text: String) -> ChunkStream {
    let chunks = vec![Ok(StreamChunk::TextDelta { text })];
    Box::pin(stream::iter(chunks))
}

fn preview(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn compose_system_prompt(
    agent_prompt: &str,
    skill_prompt: Option<&str>,
    summary_prompt: Option<&str>,
    memory_prompt: Option<&str>,
) -> String {
    let mut parts = vec![agent_prompt.trim().to_string()];
    if let Some(skill_prompt) = skill_prompt.filter(|s| !s.trim().is_empty()) {
        parts.push(skill_prompt.trim().to_string());
    }
    if let Some(summary_prompt) = summary_prompt.filter(|s| !s.trim().is_empty()) {
        parts.push(format!(
            "<conversation_summary>\nSummary of earlier conversation (older messages are no longer shown verbatim):\n{}\n</conversation_summary>",
            summary_prompt.trim()
        ));
    }
    if let Some(memory_prompt) = memory_prompt.filter(|s| !s.trim().is_empty()) {
        parts.push(memory_prompt.trim().to_string());
    }
    parts.push(RUNTIME_SYSTEM_GUARD.trim().to_string());
    parts.join("\n\n")
}

fn rows_to_messages(rows: Vec<HistoryRow>) -> Vec<LlmMessage> {
    rows.into_iter()
        .filter(|h| !matches!(h.direction, Direction::Out) || !looks_like_agent_meta_leak(&h.text))
        .map(|h| LlmMessage {
            role: match h.direction {
                Direction::In => Role::User,
                Direction::Out => Role::Assistant,
            },
            content: vec![ContentPart::Text(h.text)],
        })
        .collect()
}

fn recall_snippet(text: &str) -> String {
    let mut out: String = text.chars().take(RECALL_SNIPPET_CHARS).collect();
    if text.chars().count() > RECALL_SNIPPET_CHARS {
        out.push('…');
    }
    out.replace('\n', " ")
}

fn sanitize_final_text(text: String) -> String {
    if !looks_like_agent_meta_leak(&text) {
        return text;
    }

    let lines: Vec<&str> = text.lines().collect();
    let Some(last_meta_idx) = lines.iter().rposition(|line| meta_marker_score(line) > 0) else {
        return "처리했습니다.".to_string();
    };
    let recovered = lines[last_meta_idx + 1..]
        .iter()
        .copied()
        .filter(|line| meta_marker_score(line) == 0)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if recovered.is_empty() {
        "처리했습니다.".to_string()
    } else {
        recovered
    }
}

fn looks_like_agent_meta_leak(text: &str) -> bool {
    meta_marker_score(text) >= 2
}

fn meta_marker_score(text: &str) -> usize {
    let lower = text.to_ascii_lowercase();
    META_LEAK_MARKERS
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count()
}

#[derive(Clone, Debug)]
enum TurnMode {
    Normal,
    Schedule { tools: Vec<String> },
    Integration { tools: Vec<String> },
}

impl TurnMode {
    fn is_autonomous(&self) -> bool {
        !matches!(self, TurnMode::Normal)
    }
}

#[derive(Clone, Debug)]
struct IntegrationTurn {
    integration: IntegrationId,
    account: String,
    kind: IntegrationUpdateKind,
    external_ref: String,
    summary: String,
    observation: Option<i64>,
}

fn integration_prompt(update: &IntegrationTurn) -> String {
    let mut header = format!(
        "<integration_update integration=\"{}\" account=\"{}\" kind=\"{}\">\n{}\nexternal_ref: {}",
        update.integration,
        update.account,
        update.kind.as_str(),
        update.summary,
        update.external_ref,
    );
    if let Some(observation) = update.observation {
        let _ = write!(
            header,
            "\nobservation recorded (raw payload kept losslessly): observation:{observation}\n\
             read it back with the `observation` tool, id {observation}",
        );
    }
    format!(
        "{header}\n</integration_update>\n\
         Gather context now: pull live data with the `{}_*` tools, read what the watcher \
         actually saw with `observation`, and search prior knowledge with `memory_search`. \
         Record durable claims with `fact` in scope domain:{}, using the observation \
         reference above as source_ref. Then brief me: what happened, the key context you \
         found, and a suggested first step. Do not start the work itself.",
        update.integration, update.integration,
    )
}

fn is_schedule_tool(name: &str) -> bool {
    matches!(
        name,
        "schedule_once" | "schedule_cron" | "cancel_task" | "list_tasks"
    )
}

fn is_schedule_create_tool(name: &str) -> bool {
    matches!(name, "schedule_once" | "schedule_cron")
}

fn validate_scheduled_tool_selectors(
    arguments: &serde_json::Value,
    allowed_tools: &HashSet<String>,
) -> Result<(), String> {
    let Some(tools) = arguments.get("tools") else {
        return Ok(());
    };
    let selectors: Vec<String> = serde_json::from_value(tools.clone())
        .map_err(|e| format!("invalid tools selector list: {e}"))?;
    let known_tools = allowed_tools
        .iter()
        .filter(|name| !is_schedule_tool(name))
        .cloned()
        .collect::<Vec<_>>();
    validate_tool_selectors(&selectors, known_tools).map_err(|e| e.to_string())
}

fn truncate_for_summary(text: &str) -> String {
    const MAX: usize = 8000;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX).collect();
    out.push('…');
    out
}

const META_LEAK_MARKERS: &[&str] = &[
    "now we are to",
    "we need to respond",
    "let's craft",
    "safe approach",
    "produce final",
    "the user asked",
    "the user earlier",
    "the assistant already",
    "conversation ended",
    "last user message",
    "system expects",
    "we are chatgpt",
    "i'll respond",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn integration_turn() -> IntegrationTurn {
        IntegrationTurn {
            integration: IntegrationId::from_static("linear"),
            account: "default".into(),
            kind: IntegrationUpdateKind::Assigned,
            external_ref: "linear/default:issue:GOA-1".into(),
            summary: "GOA-1 — Fix retry storm".into(),
            observation: Some(12),
        }
    }

    #[test]
    fn integration_prompt_includes_observation() {
        let prompt = integration_prompt(&integration_turn());
        assert!(prompt.starts_with(
            "<integration_update integration=\"linear\" account=\"default\" kind=\"assigned\">"
        ));
        assert!(prompt.contains("GOA-1 — Fix retry storm"));
        assert!(prompt.contains("external_ref: linear/default:issue:GOA-1"));
        assert!(prompt.contains("observation:12"));
        assert!(prompt.contains("`linear_*` tools"));
        assert!(prompt.contains("scope domain:linear"));
        assert!(prompt.contains("Do not start the work itself"));
    }

    #[test]
    fn integration_prompt_omits_missing_observation() {
        let prompt = integration_prompt(&IntegrationTurn {
            observation: None,
            ..integration_turn()
        });
        assert!(!prompt.contains("observation recorded"));
    }

    #[test]
    fn autonomous_modes_cover_self_tick_and_integration() {
        assert!(!TurnMode::Normal.is_autonomous());
        assert!(TurnMode::Schedule { tools: vec![] }.is_autonomous());
        assert!(TurnMode::Integration { tools: vec![] }.is_autonomous());
    }

    fn selectors(values: &[&str]) -> Vec<String> {
        values
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }

    #[test]
    fn explicit_empty_agent_selector_denies_tools() {
        assert!(!selector_allows("shell", &selectors(&[])));
    }

    #[test]
    fn self_tick_empty_tool_selector_denies_tools() {
        assert!(!selector_allows_empty_denies("read", &selectors(&[])));
        assert!(selector_allows_empty_denies("read", &selectors(&["*"])));
    }

    #[test]
    fn scheduled_tool_selectors_reject_unknown_tools() {
        let allowed_tools = HashSet::from(["schedule_once".to_string(), "shell".to_string()]);
        let args = serde_json::json!({"tools": ["bash"]});

        let err = validate_scheduled_tool_selectors(&args, &allowed_tools).unwrap_err();

        assert!(err.contains("unknown tool selector"));
    }

    #[test]
    fn scheduled_tool_selectors_accept_allowed_non_schedule_tools() {
        let allowed_tools = HashSet::from([
            "schedule_once".to_string(),
            "schedule_cron".to_string(),
            "shell".to_string(),
            "read".to_string(),
        ]);
        let args = serde_json::json!({"tools": ["shell", "read"]});

        validate_scheduled_tool_selectors(&args, &allowed_tools).unwrap();
    }

    #[test]
    fn assistant_tool_call_message_contains_no_user_visible_text() {
        let calls = vec![ModelToolCall {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: serde_json::json!({"command": "ls -la"}),
        }];

        let message = assistant_tool_call_message(&calls);

        assert!(matches!(message.role, Role::Assistant));
        assert_eq!(message.content.len(), 1);
        assert!(matches!(
            &message.content[0],
            ContentPart::ToolCall { id, name, .. }
                if id == "call_1" && name == "shell"
        ));
        assert!(
            !message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Text(_)))
        );
    }

    #[test]
    fn compose_system_prompt_appends_runtime_guard() {
        let prompt = compose_system_prompt("You are dev.", None, None, None);
        assert!(prompt.contains("You are dev."));
        assert!(prompt.contains("<goat_runtime_guard>"));
        assert!(prompt.contains("Return only the final user-facing answer."));
    }

    #[test]
    fn compose_system_prompt_inserts_skill_catalog_before_runtime_guard() {
        let prompt = compose_system_prompt("You are dev.", Some("<available_skills/>"), None, None);
        let agent = prompt.find("You are dev.").unwrap();
        let skills = prompt.find("<available_skills/>").unwrap();
        let guard = prompt.find("<goat_runtime_guard>").unwrap();
        assert!(agent < skills);
        assert!(skills < guard);
    }

    #[test]
    fn compose_system_prompt_inserts_memory_before_runtime_guard() {
        let prompt = compose_system_prompt(
            "You are dev.",
            Some("<available_skills/>"),
            None,
            Some("<agent_memory>fact</agent_memory>"),
        );
        let skills = prompt.find("<available_skills/>").unwrap();
        let memory = prompt.find("<agent_memory>").unwrap();
        let guard = prompt.find("<goat_runtime_guard>").unwrap();
        assert!(skills < memory);
        assert!(memory < guard);
    }

    #[test]
    fn compose_system_prompt_inserts_summary_before_memory() {
        let prompt = compose_system_prompt(
            "You are dev.",
            None,
            Some("they talked about cats"),
            Some("<agent_memory>fact</agent_memory>"),
        );
        let summary = prompt.find("<conversation_summary>").unwrap();
        let memory = prompt.find("<agent_memory>").unwrap();
        let guard = prompt.find("<goat_runtime_guard>").unwrap();
        assert!(prompt.contains("they talked about cats"));
        assert!(summary < memory);
        assert!(memory < guard);
    }

    #[test]
    fn sanitizer_removes_agent_meta_leak_prefix() {
        let leaked = "Now we are to continue the conversation. The user asked for ls.\n\
            Let's craft the final answer.\n\
            목록 확인했습니다.\n.omx\nCargo.toml\n";

        let cleaned = sanitize_final_text(leaked.to_string());

        assert_eq!(cleaned, "목록 확인했습니다.\n.omx\nCargo.toml");
    }

    #[test]
    fn detects_common_agent_meta_leak() {
        assert!(looks_like_agent_meta_leak(
            "Now we are to continue the conversation. The user asked X. Let's craft."
        ));
        assert!(!looks_like_agent_meta_leak(
            "목록 확인했습니다.\nCargo.toml\nsrc"
        ));
    }

    #[test]
    fn schedule_tool_classification() {
        assert!(is_schedule_tool("schedule_once"));
        assert!(is_schedule_tool("schedule_cron"));
        assert!(is_schedule_tool("cancel_task"));
        assert!(is_schedule_tool("list_tasks"));
        assert!(!is_schedule_tool("recall"));
        assert!(!is_schedule_tool("shell"));
    }

    #[test]
    fn schedule_triggers_skip_guard_but_normal_does_not() {
        let schedule = TurnMode::Schedule { tools: vec![] };
        let normal = TurnMode::Normal;
        assert!(matches!(schedule, TurnMode::Schedule { .. }));
        assert!(!matches!(normal, TurnMode::Schedule { .. }));
    }

    #[test]
    fn engage_decision_table() {
        let cases = [
            (Surface::Dm, false, false, Engagement::Engage),
            (Surface::Dm, true, false, Engagement::Engage),
            (Surface::Channel, false, false, Engagement::Skip),
            (Surface::Channel, true, false, Engagement::Engage),
            (Surface::Channel, false, true, Engagement::Engage),
            (Surface::Thread, false, false, Engagement::NeedsActivity),
            (Surface::Thread, true, false, Engagement::Engage),
            (Surface::Thread, false, true, Engagement::Engage),
        ];
        for (surface, addressed, has_command, expected) in cases {
            assert_eq!(
                engage_decision(surface, addressed, has_command),
                expected,
                "surface={surface:?} addressed={addressed} has_command={has_command}"
            );
        }
    }

    #[test]
    fn parse_open_thread_args_requires_seed() {
        assert!(parse_open_thread_args(&serde_json::json!({"title": "t"})).is_none());
        assert!(parse_open_thread_args(&serde_json::json!({"title": "t", "seed": ""})).is_none());
        assert!(parse_open_thread_args(&serde_json::json!({"title": "t", "seed": "  "})).is_none());
        let ok = parse_open_thread_args(&serde_json::json!({"title": " t ", "seed": " hi "}));
        assert_eq!(ok, Some(("t".to_string(), "hi".to_string())));
    }

    #[test]
    fn thread_context_block_mentions_open_thread_only_when_offered() {
        let with = thread_context_block(Surface::Channel, true);
        assert!(with.contains("<thread_context>"));
        assert!(with.contains("shared channel"));
        assert!(with.contains("Open a new thread"));
        let without = thread_context_block(Surface::Dm, false);
        assert!(without.contains("direct message"));
        assert!(!without.contains("Open a new thread"));
    }

    #[test]
    fn surface_of_external_classifies_dm_and_channel() {
        assert_eq!(surface_of_external("dm:123"), Surface::Dm);
        assert_eq!(surface_of_external("g:1:c:2"), Surface::Channel);
    }

    use std::time::Duration;

    fn intake_thread(external: &str) -> ThreadId {
        ThreadId::new(
            goat_types::ChannelId::new("test"),
            goat_types::InstanceId::from_slug("i"),
            external,
        )
    }

    fn intake_msg(thread: ThreadId, from: &str, text: &str) -> IncomingMessage {
        IncomingMessage {
            id: MessageId(String::new()),
            agent: AgentId::from_slug("test"),
            thread,
            from: goat_types::UserHandle {
                external: from.to_string(),
                display: None,
            },
            text: text.to_string(),
            attachments: vec![],
            command: None,
            surface: Surface::Dm,
            addressed: true,
            parent: None,
            ts: chrono::Utc::now(),
            raw: serde_json::Value::Null,
        }
    }

    #[test]
    fn burst_within_debounce_coalesces_into_one() {
        let base = Instant::now();
        let mut buf = IntakeBuffer::new(Duration::from_secs(1), Duration::from_secs(5));
        let key = (intake_thread("t"), "u".to_string());
        buf.push(key.clone(), intake_msg(intake_thread("t"), "u", "a"), base);
        buf.push(
            key.clone(),
            intake_msg(intake_thread("t"), "u", "b"),
            base + Duration::from_millis(300),
        );
        buf.push(
            key.clone(),
            intake_msg(intake_thread("t"), "u", "c"),
            base + Duration::from_millis(600),
        );

        assert!(buf.drain_due(base + Duration::from_millis(1500)).is_empty());
        let due = buf.drain_due(base + Duration::from_millis(1600));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].text, "c");
        assert!(buf.next_deadline().is_none());
    }

    #[test]
    fn deliberate_pause_is_two_turns() {
        let base = Instant::now();
        let mut buf = IntakeBuffer::new(Duration::from_secs(1), Duration::from_secs(5));
        let key = (intake_thread("t"), "u".to_string());

        buf.push(
            key.clone(),
            intake_msg(intake_thread("t"), "u", "first"),
            base,
        );
        let first = buf.drain_due(base + Duration::from_secs(1));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text, "first");

        buf.push(
            key.clone(),
            intake_msg(intake_thread("t"), "u", "second"),
            base + Duration::from_secs(10),
        );
        let second = buf.drain_due(base + Duration::from_secs(11));
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].text, "second");
    }

    #[test]
    fn continuous_stream_force_flushes_at_ceiling() {
        let base = Instant::now();
        let mut buf = IntakeBuffer::new(Duration::from_secs(1), Duration::from_secs(5));
        let key = (intake_thread("t"), "u".to_string());

        let mut t = 0u64;
        while t <= 4500 {
            buf.push(
                key.clone(),
                intake_msg(intake_thread("t"), "u", "x"),
                base + Duration::from_millis(t),
            );
            t += 500;
        }

        assert_eq!(buf.next_deadline(), Some(base + Duration::from_secs(5)));
        assert!(buf.drain_due(base + Duration::from_millis(4999)).is_empty());
        assert_eq!(buf.drain_due(base + Duration::from_secs(5)).len(), 1);
    }

    #[test]
    fn distinct_keys_flush_independently() {
        let base = Instant::now();

        let mut same_thread = IntakeBuffer::new(Duration::from_secs(1), Duration::from_secs(5));
        same_thread.push(
            (intake_thread("t"), "u1".to_string()),
            intake_msg(intake_thread("t"), "u1", "a"),
            base,
        );
        same_thread.push(
            (intake_thread("t"), "u2".to_string()),
            intake_msg(intake_thread("t"), "u2", "b"),
            base,
        );
        assert_eq!(
            same_thread.drain_due(base + Duration::from_secs(1)).len(),
            2
        );

        let mut same_user = IntakeBuffer::new(Duration::from_secs(1), Duration::from_secs(5));
        same_user.push(
            (intake_thread("t1"), "u".to_string()),
            intake_msg(intake_thread("t1"), "u", "a"),
            base,
        );
        same_user.push(
            (intake_thread("t2"), "u".to_string()),
            intake_msg(intake_thread("t2"), "u", "b"),
            base,
        );
        assert_eq!(same_user.drain_due(base + Duration::from_secs(1)).len(), 2);
    }
}

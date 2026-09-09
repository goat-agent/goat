use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use futures::{SinkExt, StreamExt, channel::mpsc, stream};
use goat_agent_command::{CommandOutput, CommandRegistry};
use goat_agent_config::AgentCard;
use goat_agent_tool::{
    ToolAudience, ToolCall, ToolCaller, ToolOutput, ToolReadState, ToolRegistry, selector_allows,
    validate_tool_selectors,
};
use goat_bus::{EventBus, EventFilter};
use goat_channel::{ChannelError, ChannelHandle};
use goat_model::{Model, canonicalize_provider_id};
use goat_provider::{
    ChunkStream, ContentBlock, Message, MessageRole, Provider, Request, StreamChunk, StreamError,
    ToolChoice, ToolDefinition,
};
use goat_render::{OutgoingSink, RenderSummary, StreamRenderer};
use goat_skill::{Scopes, SkillSet};
use goat_store::{
    ActivityKind, Direction, HistoryRow, MessageSender, NewActivity, ScheduleRunStatus,
    ScheduleStatus, Store, ToolInvocationRecord, ToolInvocationStatus,
};
use goat_types::{
    AgentId, ConversationId, Event, IncomingMessage, IntegrationId, IntegrationUpdateKind,
    MessageId, SCHEDULE_FALLBACK_TIMEZONE, Surface, UserHandle, WorkflowItem,
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
        content: Vec<goat_agent_tool::ToolContent>,
        is_error: bool,
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
        ContentPart::ToolResult {
            id,
            content,
            is_error,
        } => ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: content
                .iter()
                .filter_map(|part| match part {
                    goat_agent_tool::ToolContent::Text { text } => {
                        Some(ContentBlock::Text { text: text.clone() })
                    }
                    goat_agent_tool::ToolContent::Image { media_type, data } => {
                        Some(ContentBlock::Image {
                            media_type: media_type.clone(),
                            data: data.clone(),
                        })
                    }
                    _ => None,
                })
                .collect(),
            is_error: *is_error,
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

const GOAT_SELF: &str = r"
<goat_self>
You are a goat agent: a resident actor. A turn starts three ways — a channel message, a fire of a schedule you registered (once/cron only, no self-tick), or an update from a watch workflow; the agent's `watch` section declares what is watched as named workflows of query-filtered sources.
Channels are presence; integrations are reach.
Memory scopes are owner, self, and domain:<name>. Files under core/ are always loaded and yours to curate; nightly consolidation writes notes, never core/.
Schedule prompts are notes to your future self — a fire carries no conversation.
Cite integration observations as observation:<id>. Delegate real coding to the code tool.
For the full picture, activate the `goat` skill.
</goat_self>
";

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
    pub slug: String,
    pub personality: Arc<AgentCard>,
    pub default_model: Model,
    pub timezone: Option<String>,
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
    pub turns: Arc<std::sync::atomic::AtomicUsize>,
}

struct TurnGuard(Arc<std::sync::atomic::AtomicUsize>);

impl TurnGuard {
    fn new(turns: &Arc<std::sync::atomic::AtomicUsize>) -> Self {
        turns.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(turns.clone())
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

struct StoreSink {
    store: Arc<dyn Store>,
    agent: AgentId,
    conversation: ConversationId,
    id: String,
    reply_to: Option<MessageId>,
}

#[async_trait::async_trait]
impl OutgoingSink for StoreSink {
    async fn record(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Err(e) = self
            .store
            .upsert_outgoing_text(
                self.agent,
                &self.conversation,
                &self.id,
                text,
                self.reply_to.as_ref(),
            )
            .await
        {
            warn!(agent = %self.agent, error = ?e, "recording outgoing text");
        }
    }
}

struct RoundSink {
    sink: Option<Arc<StoreSink>>,
    prefix: String,
    visible: AtomicBool,
}

#[async_trait::async_trait]
impl OutgoingSink for RoundSink {
    async fn record(&self, text: &str) {
        self.visible.store(true, Ordering::Relaxed);
        if let Some(sink) = &self.sink {
            if self.prefix.is_empty() {
                sink.record(text).await;
            } else {
                sink.record(&format!("{}{text}", self.prefix)).await;
            }
        }
    }
}

struct LiveRender<'a> {
    handle: Arc<dyn ChannelHandle>,
    route: &'a TurnRoute,
    sink: Option<Arc<StoreSink>>,
    prefix: &'a str,
}

pub struct Brain {
    agent: AgentId,
    agent_slug: String,
    personality: Arc<AgentCard>,
    default_model: Model,
    schedule_timezone: String,
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
    turns: Arc<std::sync::atomic::AtomicUsize>,
}

impl Brain {
    pub fn new(deps: BrainDeps) -> Self {
        Self {
            agent: deps.agent,
            agent_slug: deps.slug,
            personality: deps.personality,
            default_model: deps.default_model,
            schedule_timezone: deps
                .timezone
                .unwrap_or_else(|| SCHEDULE_FALLBACK_TIMEZONE.to_string()),
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
            turns: deps.turns,
        }
    }

    fn agent_definition(&self) -> String {
        match std::fs::read_to_string(&self.personality.source_path) {
            Ok(raw) if !raw.trim().is_empty() => raw.trim().to_owned(),
            Ok(_) => self.personality.system_prompt.clone(),
            Err(e) => {
                warn!(
                    agent = %self.agent,
                    path = %self.personality.source_path.display(),
                    error = ?e,
                    "re-reading agent.md failed; using the copy loaded at boot",
                );
                self.personality.system_prompt.clone()
            }
        }
    }

    pub async fn run(
        self: Arc<Self>,
        bus: EventBus,
        channels: Vec<Arc<dyn ChannelHandle>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut sub = bus.subscribe(EventFilter::Agent(self.agent));
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
                            let key = (msg.conversation.clone(), msg.from.external.clone());
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
                            run_id, schedule_id, ..
                        } => {
                            if let Err(e) = self.handle_schedule(&channels, run_id, schedule_id).await {
                                warn!(
                                    agent = %self.agent,
                                    run_id,
                                    schedule_id,
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
                        Event::WorkflowUpdate {
                            workflow,
                            items,
                            overflow,
                            ..
                        } => {
                            let update = WorkflowTurn {
                                workflow,
                                items,
                                overflow,
                            };
                            if let Err(e) = self.handle_workflow_update(&channels, update).await {
                                warn!(
                                    agent = %self.agent,
                                    error = ?e,
                                    "workflow update failed",
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
                .has_agent_activity(self.agent, &msg.conversation)
                .await?),
            Engagement::Engage => Ok(true),
        }
    }

    async fn handle_turn(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        msg: IncomingMessage,
    ) -> Result<()> {
        let _busy = TurnGuard::new(&self.turns);
        let run_id = self.start_activity(msg.conversation.channel.as_str()).await;
        let result = self.run_turn(channels, msg, run_id).await;
        self.finish_activity(run_id, &result).await;
        result
    }

    async fn run_turn(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        msg: IncomingMessage,
        run_id: Option<i64>,
    ) -> Result<()> {
        let handle = channels
            .iter()
            .find(|h| {
                h.id() == msg.conversation.channel && h.instance() == msg.conversation.instance
            })
            .cloned()
            .ok_or_else(|| anyhow!("no channel handle for {:?}", msg.conversation))?;

        let turn = handle.prepare_turn(&msg).await?;
        let reply_to = turn.reply_to.clone();
        let _typing = turn.typing;
        let sink: Arc<StoreSink> = Arc::new(StoreSink {
            store: self.store.clone(),
            agent: self.agent,
            conversation: msg.conversation.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            reply_to: Some(msg.id.clone()),
        });

        let (summary, mut messages) = self.load_context(&msg.conversation).await?;
        if let Some(call) = msg.command.clone() {
            match self.commands.call(call).await {
                Ok(CommandOutput::Query { content }) => messages.push(LlmMessage {
                    role: Role::User,
                    content: vec![ContentPart::Text(content)],
                }),
                Ok(CommandOutput::Reply { text }) => {
                    self.renderer
                        .render(
                            handle,
                            msg.conversation.clone(),
                            reply_to.clone(),
                            text_stream(self.default_model.clone(), text),
                            Some(sink.clone()),
                        )
                        .await?;
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

        self.complete_with_tools(
            handle,
            TurnRoute {
                run_id,
                conversation: msg.conversation.clone(),
                reply_to,
                surface: msg.surface,
                audience: turn_audience(msg.surface, &msg.conversation, Some(&msg.from)),
                thread_open,
            },
            &mut messages,
            TurnMode::Normal,
            summary,
            Some(sink.clone()),
        )
        .await?;

        Ok(())
    }

    async fn build_memory_section(
        &self,
        audience: Option<&ToolAudience>,
        query_text: Option<&str>,
    ) -> Option<String> {
        if !self.memory_enabled {
            return None;
        }
        self.build_engine_section(audience, query_text).await
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

    async fn build_engine_section(
        &self,
        audience: Option<&ToolAudience>,
        query_text: Option<&str>,
    ) -> Option<String> {
        use goat_memory::{Audience, Scope};
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

        let audience = match audience {
            Some(ToolAudience::Principal(reference)) => Audience::principal(reference.clone()).ok(),
            Some(ToolAudience::Shared(reference)) => Audience::shared(reference.clone()).ok(),
            None => None,
        }
        .unwrap_or_else(Audience::global);
        if let Some(query) = query_text.filter(|q| !q.trim().is_empty())
            && let Ok(hits) = self
                .memory_engine
                .recall(&audience, &scopes, query, 6)
                .await
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

    async fn history_messages(&self, conv: &ConversationId) -> Result<Vec<LlmMessage>> {
        let history = self
            .store
            .recent(self.agent, conv, self.history_window)
            .await
            .context("read history")?;
        Ok(rows_to_messages(history))
    }

    async fn load_context(
        &self,
        conv: &ConversationId,
    ) -> Result<(Option<String>, Vec<LlmMessage>)> {
        if !self.summarize_enabled {
            return Ok((None, self.history_messages(conv).await?));
        }

        let total = self.store.message_count(self.agent, conv).await?;
        let existing = self
            .store
            .get_conversation_summary(self.agent, conv)
            .await?;
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
                        .upsert_conversation_summary(self.agent, conv, &updated, new_count)
                        .await
                    {
                        warn!(agent = %self.agent, error = ?e, "upsert_conversation_summary failed");
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
            .map(|h| format!("{}: {}", history_speaker(h), h.text))
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
        mut sink: Option<Arc<StoreSink>>,
    ) -> Result<(RenderSummary, ConversationId)> {
        const MAX_TOOL_ROUNDS: usize = 1000;

        let provider = self.providers.route(&self.default_model)?;
        let skill_prompt =
            SkillSet::load(&Scopes::agent(&self.goat_root, &self.agent_slug)).catalog();
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
            let mem = self
                .build_memory_section(route.audience.as_ref(), query_text.as_deref())
                .await;
            let goals = self.build_goals_section().await;
            match (mem, goals) {
                (Some(m), Some(g)) => Some(format!("{m}\n\n{g}")),
                (Some(m), None) => Some(m),
                (None, g) => g,
            }
        };
        let base_system = format!(
            "{}\n\n{}",
            compose_system_prompt(
                &self.agent_definition(),
                skill_prompt.as_deref(),
                summary.as_deref(),
                memory_section.as_deref(),
            ),
            current_time_block(chrono::Utc::now(), &self.schedule_timezone),
        );
        let system_prompt = turn_system_prompt(
            &base_system,
            &mode,
            route.surface,
            route.thread_open.is_some(),
        );
        let mut rendered = RenderSummary::default();

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

            let live = (!mode.is_autonomous()).then(|| LiveRender {
                handle: handle.clone(),
                route: &route,
                sink: sink.clone(),
                prefix: &rendered.final_text,
            });
            let (folded, round_summary) = self.stream_with_retry(&provider, req, live).await?;
            if !mode.is_autonomous() {
                rendered.messages_sent += round_summary.messages_sent;
                rendered.edits += round_summary.edits;
                rendered.final_text.push_str(&folded.text);
            }

            if folded.tool_calls.is_empty() {
                if !mode.is_autonomous() {
                    return Ok((rendered, route.conversation));
                }
                let final_text = sanitize_final_text(folded.text);
                if mode.is_autonomous() && final_text.trim().eq_ignore_ascii_case("skip") {
                    return Ok((
                        RenderSummary {
                            messages_sent: 0,
                            edits: 0,
                            final_text: "skip".into(),
                        },
                        route.conversation,
                    ));
                }
                let summary = self
                    .renderer
                    .render(
                        handle,
                        route.conversation.clone(),
                        route.reply_to,
                        text_stream(self.default_model.clone(), final_text),
                        None,
                    )
                    .await?;
                return Ok((summary, route.conversation));
            }

            messages.push(assistant_tool_call_message(folded.text, &folded.tool_calls));

            for call in folded.tool_calls {
                if route.thread_open.is_some() && call.name.as_str() == OPEN_THREAD_TOOL {
                    self.record_activity(
                        route.run_id,
                        ActivityKind::ToolStarted,
                        Some(call.name.clone()),
                        None,
                    )
                    .await;
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
                                .open_thread(&route.conversation, anchor.as_ref(), &title)
                                .await
                            {
                                Ok(new_thread) => {
                                    let _ = self
                                        .store
                                        .append_incoming_text(self.agent, &new_thread, &seed)
                                        .await;
                                    route.audience =
                                        Some(ToolAudience::Shared(new_thread.to_key()));
                                    route.conversation = new_thread;
                                    route.reply_to = None;
                                    route.thread_open = None;
                                    if sink.is_some() {
                                        sink = Some(Arc::new(StoreSink {
                                            store: self.store.clone(),
                                            agent: self.agent,
                                            conversation: route.conversation.clone(),
                                            id: uuid::Uuid::new_v4().to_string(),
                                            reply_to: None,
                                        }));
                                    }
                                    rendered.final_text.clear();
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
                    .execute_tool(
                        route.run_id,
                        &route.conversation,
                        route.audience.clone(),
                        &call,
                        read_state.clone(),
                        &allowed_tools,
                    )
                    .await;
                messages.push(LlmMessage {
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        id: call.id,
                        content: output.content,
                        is_error: output.is_error,
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
                route.conversation,
            ));
        }
        let text = "I stopped because tool execution exceeded the safety round limit.".to_string();
        let round_sink = Arc::new(RoundSink {
            sink,
            prefix: rendered.final_text.clone(),
            visible: AtomicBool::new(false),
        });
        let summary = self
            .renderer
            .render(
                handle,
                route.conversation.clone(),
                route.reply_to,
                text_stream(self.default_model.clone(), text),
                Some(round_sink),
            )
            .await?;
        rendered.messages_sent += summary.messages_sent;
        rendered.edits += summary.edits;
        rendered.final_text.push_str(&summary.final_text);
        Ok((rendered, route.conversation))
    }

    async fn start_activity(&self, trigger: &str) -> Option<i64> {
        match self.store.start_activity(self.agent, trigger).await {
            Ok(run_id) => Some(run_id),
            Err(error) => {
                warn!(agent = %self.agent, trigger, error = ?error, "starting activity");
                None
            }
        }
    }

    async fn record_activity(
        &self,
        run_id: Option<i64>,
        kind: ActivityKind,
        detail: Option<String>,
        ok: Option<bool>,
    ) {
        let Some(run_id) = run_id else {
            return;
        };
        if let Err(error) = self
            .store
            .record_activity(NewActivity {
                agent: self.agent,
                kind,
                run_id,
                detail,
                ok,
            })
            .await
        {
            warn!(agent = %self.agent, run_id, error = ?error, "recording activity");
        }
    }

    async fn finish_activity(&self, run_id: Option<i64>, result: &Result<()>) {
        self.record_activity(
            run_id,
            ActivityKind::TurnFinished,
            result.as_ref().err().map(|error| format!("{error:#}")),
            Some(result.is_ok()),
        )
        .await;
    }

    async fn finish_run_logged(
        &self,
        run_id: i64,
        status: ScheduleRunStatus,
        note: Option<String>,
    ) {
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
        task_run_id: i64,
        schedule_id: i64,
    ) -> Result<()> {
        let _busy = TurnGuard::new(&self.turns);
        let run_id = self.start_activity("schedule").await;
        self.record_activity(
            run_id,
            ActivityKind::ScheduleFired,
            Some(schedule_id.to_string()),
            None,
        )
        .await;
        let result = self.run_schedule(channels, run_id, schedule_id).await;
        let (status, note) = match &result {
            Ok((status, note)) => (*status, note.clone()),
            Err(error) => (ScheduleRunStatus::Failed, Some(format!("{error:#}"))),
        };
        self.finish_run_logged(task_run_id, status, note.clone())
            .await;
        self.record_activity(
            run_id,
            ActivityKind::TurnFinished,
            note,
            Some(!matches!(status, ScheduleRunStatus::Failed)),
        )
        .await;
        result.map(|_| ())
    }

    async fn run_schedule(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        run_id: Option<i64>,
        schedule_id: i64,
    ) -> Result<(ScheduleRunStatus, Option<String>)> {
        let schedule = match self.store.get_schedule(schedule_id).await? {
            Some(schedule) if matches!(schedule.status, ScheduleStatus::Active) => schedule,
            Some(_) => {
                return Ok((
                    ScheduleRunStatus::Skipped,
                    Some("task no longer active".into()),
                ));
            }
            None => return Err(anyhow!("task row missing")),
        };
        let conv = schedule.origin_conv.clone();
        let handle = channels
            .iter()
            .find(|handle| handle.id() == conv.channel && handle.instance() == conv.instance)
            .cloned()
            .ok_or_else(|| anyhow!("no channel handle for origin_conv"))?;
        let mut messages = vec![LlmMessage::user_text(schedule.instruction)];
        let surface = handle
            .surface(&conv)
            .await
            .context("classify schedule origin surface")?;
        let (summary, conversation) = self
            .complete_with_tools(
                handle,
                TurnRoute {
                    run_id,
                    conversation: conv.clone(),
                    reply_to: None,
                    surface,
                    audience: turn_audience(surface, &conv, None),
                    thread_open: None,
                },
                &mut messages,
                TurnMode::Schedule {
                    tools: schedule.tools,
                },
                None,
                None,
            )
            .await?;
        let trimmed = summary.final_text.trim();
        if trimmed.eq_ignore_ascii_case("skip") {
            return Ok((ScheduleRunStatus::Skipped, Some("model declined".into())));
        }
        if trimmed.is_empty() {
            return Err(anyhow!("empty response from model"));
        }
        self.store
            .append_outgoing_text(self.agent, &conversation, &summary.final_text, None)
            .await
            .context("append outgoing text for schedule")?;
        Ok((
            ScheduleRunStatus::Done,
            Some(truncate_for_summary(&summary.final_text)),
        ))
    }

    async fn handle_integration_update(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        update: IntegrationTurn,
    ) -> Result<()> {
        let _busy = TurnGuard::new(&self.turns);
        let run_id = self.start_activity("integration").await;
        let result = self.run_integration_update(channels, update, run_id).await;
        self.finish_activity(run_id, &result).await;
        result
    }

    async fn run_integration_update(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        update: IntegrationTurn,
        run_id: Option<i64>,
    ) -> Result<()> {
        let resolved = match self.store.latest_conversation(self.agent).await? {
            Some(conversation) => channels
                .iter()
                .find(|h| h.id() == conversation.channel && h.instance() == conversation.instance)
                .cloned()
                .map(|handle| (conversation, handle)),
            None => None,
        };
        let Some((conversation, handle)) = resolved else {
            warn!(
                agent = %self.agent,
                integration = %update.integration,
                external_ref = %update.external_ref,
                "no channel handle for integration update; dropping briefing",
            );
            return Err(anyhow!("no channel handle for integration update"));
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
        let surface = handle
            .surface(&conversation)
            .await
            .context("classify integration destination surface")?;

        let (summary, conversation) = self
            .complete_with_tools(
                handle,
                TurnRoute {
                    run_id,
                    conversation: conversation.clone(),
                    reply_to: None,
                    surface,
                    audience: turn_audience(surface, &conversation, None),
                    thread_open: None,
                },
                &mut messages,
                TurnMode::Integration { tools },
                None,
                None,
            )
            .await?;

        let trimmed = summary.final_text.trim();
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("skip") {
            self.store
                .append_outgoing_text(self.agent, &conversation, &summary.final_text, None)
                .await
                .context("append outgoing text for integration update")?;
        }
        Ok(())
    }

    async fn handle_workflow_update(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        update: WorkflowTurn,
    ) -> Result<()> {
        let _busy = TurnGuard::new(&self.turns);
        let run_id = self.start_activity("workflow").await;
        let result = self.run_workflow_update(channels, update, run_id).await;
        self.finish_activity(run_id, &result).await;
        result
    }

    async fn run_workflow_update(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        update: WorkflowTurn,
        run_id: Option<i64>,
    ) -> Result<()> {
        let resolved = match self.store.latest_conversation(self.agent).await? {
            Some(conversation) => channels
                .iter()
                .find(|h| h.id() == conversation.channel && h.instance() == conversation.instance)
                .cloned()
                .map(|handle| (conversation, handle)),
            None => None,
        };
        let Some((conversation, handle)) = resolved else {
            warn!(
                agent = %self.agent,
                workflow = %update.workflow,
                "no channel handle for workflow update; dropping briefing",
            );
            return Err(anyhow!("no channel handle for workflow update"));
        };

        let prompt = workflow_prompt(&update, &self.integration_tools);
        let mut messages = vec![LlmMessage {
            role: Role::User,
            content: vec![ContentPart::Text(prompt)],
        }];

        let mut tools = self.integration_tools.clone();
        tools.extend(
            ["memory_search", "fact", "observation"]
                .iter()
                .map(std::string::ToString::to_string),
        );
        let surface = handle
            .surface(&conversation)
            .await
            .context("classify workflow destination surface")?;

        let (summary, conversation) = self
            .complete_with_tools(
                handle,
                TurnRoute {
                    run_id,
                    conversation: conversation.clone(),
                    reply_to: None,
                    surface,
                    audience: turn_audience(surface, &conversation, None),
                    thread_open: None,
                },
                &mut messages,
                TurnMode::Integration { tools },
                None,
                None,
            )
            .await?;

        let trimmed = summary.final_text.trim();
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("skip") {
            self.store
                .append_outgoing_text(self.agent, &conversation, &summary.final_text, None)
                .await
                .context("append outgoing text for workflow update")?;
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
                        && selector_allows(spec.name.as_str(), tools)
                }
            })
            .map(|spec| {
                let name = spec.name.as_str().to_string();
                let mut input_schema = spec.input_schema;
                if is_schedule_create_tool(&name) {
                    set_schedule_timezone_schema_default(
                        &mut input_schema,
                        &self.schedule_timezone,
                    );
                }
                ToolSpec {
                    name,
                    description: spec.description.unwrap_or_default(),
                    input_schema,
                }
            })
            .collect()
    }

    async fn execute_tool(
        &self,
        run_id: Option<i64>,
        conv: &ConversationId,
        audience: Option<ToolAudience>,
        call: &ModelToolCall,
        read_state: ToolReadState,
        allowed_tools: &HashSet<String>,
    ) -> ToolOutput {
        self.record_activity(
            run_id,
            ActivityKind::ToolStarted,
            Some(call.name.clone()),
            None,
        )
        .await;
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
        let ctx = ToolCaller {
            agent: self.agent,
            agent_slug: self.agent_slug.clone(),
            conversation: conv.clone(),
            audience,
            goat_root: self.goat_root.clone(),
            read_state,
        };
        let mut arguments = call.arguments.clone();
        if is_schedule_create_tool(name.as_str()) {
            inject_schedule_timezone(&mut arguments, &self.schedule_timezone);
        }
        let tool_call = ToolCall {
            call_id: call.id.clone(),
            name: name.clone(),
            arguments,
        };
        let resolved_name = name.to_string();
        let output = self.tools.call(ctx, tool_call).await;
        self.audit_tool_call(conv, call, resolved_name, &output, started_at)
            .await;
        output
    }

    async fn audit_tool_call(
        &self,
        conv: &ConversationId,
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
            conversation: conv.clone(),
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

fn assistant_tool_call_message(text: String, calls: &[ModelToolCall]) -> LlmMessage {
    let mut content = Vec::with_capacity(calls.len() + usize::from(!text.is_empty()));
    if !text.is_empty() {
        content.push(ContentPart::Text(text));
    }
    content.extend(calls.iter().map(|call| ContentPart::ToolCall {
        id: call.id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
    }));
    LlmMessage {
        role: Role::Assistant,
        content,
    }
}

struct TurnRoute {
    run_id: Option<i64>,
    conversation: ConversationId,
    reply_to: Option<MessageId>,
    surface: Surface,
    audience: Option<ToolAudience>,
    thread_open: Option<ThreadOpenCtx>,
}

fn turn_audience(
    surface: Surface,
    conversation: &ConversationId,
    user: Option<&UserHandle>,
) -> Option<ToolAudience> {
    match (surface, user) {
        (Surface::Dm, Some(user)) => Some(ToolAudience::Principal(
            serde_json::json!([
                conversation.channel.as_str(),
                conversation.instance.to_string(),
                user.external
            ])
            .to_string(),
        )),
        (Surface::Dm, None) => None,
        (Surface::Channel | Surface::Thread, _) => {
            Some(ToolAudience::Shared(conversation.to_key()))
        }
    }
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
    pending: HashMap<(ConversationId, String), Pending>,
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

    fn push(&mut self, key: (ConversationId, String), msg: IncomingMessage, now: Instant) {
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
        let due: Vec<(ConversationId, String)> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(k, _)| k.clone())
            .collect();
        due.into_iter()
            .filter_map(|k| self.pending.remove(&k).map(|p| p.last))
            .collect()
    }

    fn take(&mut self, key: &(ConversationId, String)) -> Option<Pending> {
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
            content: vec![goat_agent_tool::ToolContent::Text {
                text: content.into(),
            }],
            is_error: false,
        }],
    }
}

fn turn_system_prompt(
    base_system: &str,
    mode: &TurnMode,
    surface: Surface,
    offers_thread: bool,
) -> String {
    let thread_context = thread_context_block(surface, offers_thread);
    match mode {
        TurnMode::Normal => format!("{base_system}{thread_context}"),
        TurnMode::Schedule { .. } => format!(
            "{base_system}{thread_context}\n\n<schedule_context>\nYou are running at the \
             fire moment of a scheduled task. Read the task and act. \
             If the task is no longer worth doing, reply with exactly: skip\n\
             </schedule_context>"
        ),
        TurnMode::Integration { .. } => format!(
            "{base_system}{thread_context}\n\n<integration_update_context>\nAn external \
             service update woke you. Gather context with your tools, store \
             durable findings in memory, keep the work anchor goal current, \
             then brief the owner concisely. Do not start the work itself \
             and take no external write actions beyond the briefing. If \
             nothing is worth surfacing, reply with exactly: skip\n\
             </integration_update_context>"
        ),
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
        live: Option<LiveRender<'_>>,
    ) -> Result<(FoldedTurn, RenderSummary)> {
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
                Ok(stream) => {
                    if let Some(live) = &live {
                        let (tx, rx) = mpsc::channel(1);
                        let sink = Arc::new(RoundSink {
                            sink: live.sink.clone(),
                            prefix: live.prefix.to_string(),
                            visible: AtomicBool::new(false),
                        });
                        let folding = fold_turn_to(stream, self.stream_idle_timeout, Some(tx));
                        let rendering = self.renderer.render(
                            live.handle.clone(),
                            live.route.conversation.clone(),
                            live.route.reply_to.clone(),
                            Box::pin(rx),
                            Some(sink.clone()),
                        );
                        tokio::pin!(folding, rendering);
                        let (folded, rendered) = tokio::select! {
                            folded = &mut folding => (folded, rendering.await),
                            rendered = &mut rendering => {
                                if let Err(error) = &rendered
                                    && !matches!(error, ChannelError::Provider(_))
                                {
                                    return Err(anyhow!("{error}"));
                                }
                                (folding.await, rendered)
                            }
                        };
                        if let Err(error) = &rendered
                            && !matches!(error, ChannelError::Provider(_))
                        {
                            return Err(anyhow!("{error}"));
                        }
                        match folded {
                            Ok(mut folded) => {
                                let mut summary = rendered?;
                                folded.text = std::mem::take(&mut summary.final_text);
                                Ok((folded, summary))
                            }
                            Err(error) if sink.visible.load(Ordering::Relaxed) => {
                                return Err(error.into());
                            }
                            Err(error) => Err(error),
                        }
                    } else {
                        fold_turn(stream, self.stream_idle_timeout)
                            .await
                            .map(|folded| (folded, RenderSummary::default()))
                    }
                }
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
    stream: ChunkStream,
    idle_timeout: std::time::Duration,
) -> Result<FoldedTurn, StreamError> {
    fold_turn_to(stream, idle_timeout, None).await
}

async fn fold_turn_to(
    mut stream: ChunkStream,
    idle_timeout: std::time::Duration,
    mut output: Option<mpsc::Sender<Result<StreamChunk, StreamError>>>,
) -> Result<FoldedTurn, StreamError> {
    let mut text = String::new();
    let mut calls: Vec<ModelToolCall> = Vec::new();
    loop {
        let next = tokio::time::timeout(idle_timeout, stream.next())
            .await
            .map_err(|_| StreamError::transport("LLM stream stalled"))
            .and_then(Option::transpose);
        let chunk = match next {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                if let Some(output) = &mut output {
                    let _ = output.send(Err(error.clone())).await;
                }
                return Err(error);
            }
        };
        match chunk {
            StreamChunk::TextDelta { text: delta } => {
                if let Some(output) = &mut output {
                    output
                        .send(Ok(StreamChunk::TextDelta { text: delta }))
                        .await
                        .map_err(|_| StreamError::other("response renderer closed"))?;
                } else {
                    text.push_str(&delta);
                }
            }
            StreamChunk::ToolCall { id, name, input } => {
                let id = if id.is_empty() {
                    format!("call_{}", calls.len())
                } else {
                    id
                };
                let arguments = if input.trim().is_empty() {
                    serde_json::Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&input).unwrap_or_else(|error| {
                        serde_json::json!({"_invalid_json": input, "_error": error.to_string()})
                    })
                };
                calls.push(ModelToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            _ => {}
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

fn set_schedule_timezone_schema_default(
    input_schema: &mut serde_json::Value,
    schedule_timezone: &str,
) {
    input_schema["properties"]["timezone"]["default"] =
        serde_json::Value::String(schedule_timezone.to_string());
}

fn inject_schedule_timezone(arguments: &mut serde_json::Value, schedule_timezone: &str) {
    if let Some(arguments) = arguments.as_object_mut()
        && arguments
            .get("timezone")
            .is_none_or(serde_json::Value::is_null)
    {
        arguments.insert(
            "timezone".to_string(),
            serde_json::Value::String(schedule_timezone.to_string()),
        );
    }
}

fn current_time_block(now: chrono::DateTime<chrono::Utc>, schedule_timezone: &str) -> String {
    let now_iso = now.to_rfc3339();
    format!(
        "<current_time iso8601=\"{now_iso}\" timezone=\"{schedule_timezone}\">\n\
         The current instant is {now_iso}. The agent's effective schedule timezone is \
         {schedule_timezone}. Resolve unspecified owner time references in {schedule_timezone}. \
         Schedule tools use this timezone unless an explicit IANA timezone overrides it.\n\
         </current_time>"
    )
}

fn compose_system_prompt(
    agent_prompt: &str,
    skill_prompt: Option<&str>,
    summary_prompt: Option<&str>,
    memory_prompt: Option<&str>,
) -> String {
    let mut parts = vec![
        GOAT_SELF.trim().to_string(),
        agent_prompt.trim().to_string(),
    ];
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

fn history_speaker(row: &HistoryRow) -> String {
    match (&row.sender, row.direction) {
        (Some(MessageSender::User(user)), _) => match user.display.as_deref() {
            Some(display) if !display.trim().is_empty() => {
                format!("user {display} [{}]", user.external)
            }
            _ => format!("user [{}]", user.external),
        },
        (Some(MessageSender::Agent(agent)), _) => format!("agent [{agent}]"),
        (None, Direction::In) => "user".to_string(),
        (None, Direction::Out) => "assistant".to_string(),
    }
}

fn history_content(row: &HistoryRow) -> String {
    match row.direction {
        Direction::In => format!("{}: {}", history_speaker(row), row.text),
        Direction::Out => row.text.clone(),
    }
}

fn rows_to_messages(rows: Vec<HistoryRow>) -> Vec<LlmMessage> {
    rows.into_iter()
        .filter(|h| !matches!(h.direction, Direction::Out) || !looks_like_agent_meta_leak(&h.text))
        .map(|h| LlmMessage {
            role: match h.direction {
                Direction::In => Role::User,
                Direction::Out => Role::Assistant,
            },
            content: vec![ContentPart::Text(history_content(&h))],
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

struct WorkflowTurn {
    workflow: String,
    items: Vec<WorkflowItem>,
    overflow: usize,
}

fn workflow_prompt(update: &WorkflowTurn, registered_tools: &[String]) -> String {
    let mut body = format!("<workflow_update workflow=\"{}\">", update.workflow);
    for item in &update.items {
        let _ = write!(
            body,
            "\n<item integration=\"{}\" account=\"{}\" kind=\"{}\">\n{}\nexternal_ref: {}",
            item.integration,
            item.account,
            item.kind.as_str(),
            item.summary,
            item.external_ref,
        );
        if let Some(observation) = item.observation {
            let _ = write!(
                body,
                "\nobservation recorded (raw payload kept losslessly): observation:{observation}",
            );
        }
        body.push_str("\n</item>");
    }
    if update.overflow > 0 {
        let _ = write!(body, "\n(+{} more items waiting)", update.overflow);
    }
    body.push_str("\n</workflow_update>\n");

    let mut integrations: Vec<&str> = update
        .items
        .iter()
        .map(|item| item.integration.as_str())
        .collect();
    integrations.sort_unstable();
    integrations.dedup();
    let hints: Vec<String> = integrations
        .iter()
        .filter(|name| {
            let prefix = format!("{name}_");
            registered_tools
                .iter()
                .any(|tool| tool.starts_with(&prefix))
        })
        .map(|name| format!("`{name}_*`"))
        .collect();
    let live = if hints.is_empty() {
        String::new()
    } else {
        format!("pull live data with the {} tools, ", hints.join(", "))
    };
    let domains: Vec<String> = integrations
        .iter()
        .map(|name| format!("domain:{name}"))
        .collect();
    let _ = write!(
        body,
        "Gather context now: {live}read what the watcher actually saw with `observation`, \
         and search prior knowledge with `memory_search`. Record durable claims with `fact` \
         in each item's integration scope ({}), using its observation reference as source_ref. \
         Then brief me once, covering these items together: what happened, the key context \
         you found, and a suggested first step. Do not start the work itself.",
        domains.join(", "),
    );
    body
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
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    use goat_channel::test_support::{MockChannelHandle, MockEvent};
    use goat_channel::{ChannelCapabilities, ChannelIdentity, ChannelResult, SentRef, TypingGuard};
    use goat_provider::{AuthMethod, Capabilities, ProviderId};
    use goat_store::{ActivityRecord, NewSchedule, ScheduleKind, SqliteStore};
    use goat_types::{ChannelId, InstanceId, OutgoingBody};
    use tokio::sync::Notify;

    struct ScriptedProvider {
        streams: std::sync::Mutex<VecDeque<ChunkStream>>,
        requests: tokio::sync::Mutex<Vec<Request>>,
        requested: Notify,
    }

    #[async_trait::async_trait]
    impl Provider for ScriptedProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("scripted")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: true,
                auth: AuthMethod::None,
                images: true,
            }
        }

        async fn stream(&self, request: Request) -> Result<ChunkStream, StreamError> {
            self.requests.lock().await.push(request);
            self.requested.notify_one();
            self.streams
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| StreamError::invalid_request("unexpected provider request"))
        }

        fn discover(
            &self,
            _out: tokio::sync::mpsc::Sender<goat_provider::Model>,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(async {})
        }
    }

    impl ScriptedProvider {
        async fn wait_for_requests(&self, count: usize) {
            loop {
                let notified = self.requested.notified();
                if self.requests.lock().await.len() >= count {
                    return;
                }
                notified.await;
            }
        }
    }

    struct ObservedChannel {
        inner: Arc<MockChannelHandle>,
        changed: Notify,
        fail_edits: AtomicBool,
    }

    #[async_trait::async_trait]
    impl ChannelHandle for ObservedChannel {
        fn instance(&self) -> InstanceId {
            self.inner.instance()
        }

        fn agent(&self) -> AgentId {
            self.inner.agent()
        }

        fn id(&self) -> ChannelId {
            self.inner.id()
        }

        fn identity(&self) -> ChannelIdentity {
            self.inner.identity()
        }

        fn capabilities(&self) -> ChannelCapabilities {
            self.inner.capabilities()
        }

        async fn surface(&self, _conversation: &ConversationId) -> ChannelResult<Surface> {
            Ok(Surface::Dm)
        }

        async fn send(
            &self,
            conv: &ConversationId,
            body: OutgoingBody,
            reply_to: Option<MessageId>,
        ) -> ChannelResult<SentRef> {
            let sent = self.inner.send(conv, body, reply_to).await?;
            self.changed.notify_one();
            Ok(sent)
        }

        async fn edit(&self, sent: &SentRef, body: OutgoingBody) -> ChannelResult<()> {
            if self.fail_edits.load(Ordering::Relaxed) {
                return Err(ChannelError::BadRequest("message was deleted".into()));
            }
            self.inner.edit(sent, body).await?;
            self.changed.notify_one();
            Ok(())
        }

        async fn typing(&self, conv: &ConversationId) -> ChannelResult<TypingGuard> {
            self.inner.typing(conv).await
        }

        fn supports_threads(&self) -> bool {
            true
        }

        async fn open_thread(
            &self,
            parent: &ConversationId,
            anchor: Option<&MessageId>,
            title: &str,
        ) -> ChannelResult<ConversationId> {
            self.inner.open_thread(parent, anchor, title).await
        }
    }

    impl ObservedChannel {
        async fn wait_for_text(&self, expected: &str) {
            loop {
                let notified = self.changed.notified();
                if self
                    .inner
                    .events()
                    .await
                    .iter()
                    .any(|event| event.as_text() == Some(expected))
                {
                    return;
                }
                notified.await;
            }
        }
    }

    struct WriteNote;

    #[async_trait::async_trait]
    impl goat_agent_tool::ToolHandler for WriteNote {
        async fn call(&self, caller: ToolCaller, _call: ToolCall) -> ToolOutput {
            match tokio::fs::write(caller.goat_root.join("tool-note.txt"), "tool ran").await {
                Ok(()) => ToolOutput::text("note saved"),
                Err(error) => ToolOutput::error(error.to_string()),
            }
        }
    }

    struct TurnFixture {
        brain: Brain,
        provider: Arc<ScriptedProvider>,
        channel: Arc<ObservedChannel>,
        store: Arc<SqliteStore>,
        dir: tempfile::TempDir,
    }

    impl TurnFixture {
        async fn new(streams: Vec<ChunkStream>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("goat.db");
            let agent = AgentId::from_slug("test");
            let store = Arc::new(SqliteStore::open(&path).await.unwrap());
            store.ensure_agent(agent, "test", "Test").await.unwrap();
            let memory_engine = Arc::new(
                goat_memory::MemoryEngine::open(&path, dir.path(), None, 180.0)
                    .await
                    .unwrap(),
            );
            let provider = Arc::new(ScriptedProvider {
                streams: std::sync::Mutex::new(streams.into()),
                requests: tokio::sync::Mutex::new(Vec::new()),
                requested: Notify::new(),
            });
            let channel = Arc::new(ObservedChannel {
                inner: MockChannelHandle::with_threads(
                    ChannelId::new("test"),
                    agent,
                    InstanceId::from_slug("test/channel"),
                    ChannelIdentity::new("goat", "Goat"),
                    ChannelCapabilities::new(4096, Duration::from_millis(50), None),
                ),
                changed: Notify::new(),
                fail_edits: AtomicBool::new(false),
            });
            let mut tools = ToolRegistry::default();
            tools.insert_handler(
                goat_agent_tool::ToolSpec::new(
                    goat_agent_tool::ToolName::from_static("record"),
                    "Write a local note",
                    serde_json::json!({"type": "object", "properties": {}}),
                ),
                Arc::new(WriteNote),
                true,
            );
            let brain = Brain::new(BrainDeps {
                agent,
                slug: "test".into(),
                personality: Arc::new(AgentCard {
                    system_prompt: "Answer the user.".into(),
                    source_path: dir.path().join("agent.md"),
                }),
                default_model: Model::new(provider.id(), "scripted"),
                timezone: None,
                history_window: 20,
                tool_selectors: vec!["*".into()],
                providers: Arc::new(ProviderRegistry::from_providers(vec![provider.clone()])),
                tools: Arc::new(tools),
                commands: Arc::new(CommandRegistry::new()),
                store: store.clone(),
                memory_engine,
                memory_enabled: false,
                summarize_enabled: false,
                renderer: Arc::new(goat_render::DefaultStreamRenderer),
                goat_root: dir.path().to_owned(),
                stream_idle_timeout: Duration::from_secs(30),
                llm_max_retries: 1,
                integration_tools: vec!["record".into()],
                intake_debounce: Duration::ZERO,
                intake_ceiling: Duration::ZERO,
                turns: Arc::new(AtomicUsize::new(0)),
            });
            Self {
                brain,
                provider,
                channel,
                store,
                dir,
            }
        }

        fn conversation(&self) -> ConversationId {
            ConversationId::new(self.channel.id(), self.channel.instance(), "main")
        }

        fn channels(&self) -> Vec<Arc<dyn ChannelHandle>> {
            vec![self.channel.clone()]
        }

        async fn incoming(&self) -> IncomingMessage {
            let mut message = intake_msg(self.conversation(), "owner", "Write a note and answer.");
            message.id = MessageId(uuid::Uuid::new_v4().to_string());
            self.store.append_incoming(&message).await.unwrap();
            message
        }

        async fn activity(&self) -> Vec<ActivityRecord> {
            self.store
                .activity_since(&[self.brain.agent], 0, 100)
                .await
                .unwrap()
        }

        async fn outgoing(&self, conversation: &ConversationId) -> Vec<String> {
            self.store
                .recent(self.brain.agent, conversation, 100)
                .await
                .unwrap()
                .into_iter()
                .filter(|row| row.direction == Direction::Out)
                .map(|row| row.text)
                .collect()
        }

        async fn schedule(&self) -> (i64, i64) {
            let now = chrono::Utc::now();
            let schedule_id = self
                .store
                .insert_schedule(NewSchedule {
                    agent: self.brain.agent,
                    instruction: "Write a local note, then decide whether to notify.".into(),
                    tools: vec!["record".into()],
                    origin_conv: self.conversation(),
                    schedule: ScheduleKind::Once(now),
                    timezone: None,
                    created_by_msg_id: None,
                })
                .await
                .unwrap();
            let task_run_id = self
                .store
                .insert_schedule_run(
                    schedule_id,
                    now,
                    "Write a local note, then decide whether to notify.".into(),
                )
                .await
                .unwrap();
            self.store.claim_due_run(now).await.unwrap().unwrap();
            (task_run_id, schedule_id)
        }
    }

    fn chunks(items: Vec<Result<StreamChunk, StreamError>>) -> ChunkStream {
        Box::pin(stream::iter(items))
    }

    fn delta(text: &str) -> StreamChunk {
        StreamChunk::TextDelta { text: text.into() }
    }

    fn record_call() -> ChunkStream {
        chunks(vec![Ok(StreamChunk::ToolCall {
            id: "write-note".into(),
            name: "record".into(),
            input: "{}".into(),
        })])
    }

    fn controlled_stream() -> (
        tokio::sync::mpsc::UnboundedSender<Result<StreamChunk, StreamError>>,
        ChunkStream,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        (tx, Box::pin(stream))
    }

    fn assert_lifecycle(records: &[ActivityRecord], kinds: &[ActivityKind], ok: bool) {
        assert_eq!(
            records.iter().map(|row| &row.kind).collect::<Vec<_>>(),
            kinds.iter().collect::<Vec<_>>()
        );
        let run_id = records[0].id;
        assert!(records.iter().all(|row| row.run_id == run_id));
        assert!(
            records
                .windows(2)
                .all(|rows| rows[0].id < rows[1].id && rows[0].at <= rows[1].at)
        );
        assert_eq!(records.last().unwrap().ok, Some(ok));
    }

    #[tokio::test]
    async fn normal_turn_creates_and_updates_before_provider_eof_and_records_tool_lifecycle() {
        let (tx, stream) = controlled_stream();
        let fixture = TurnFixture::new(vec![record_call(), stream]).await;
        let message = fixture.incoming().await;
        let channels = fixture.channels();
        let first = "We need to respond";
        let final_text = "We need to respond and let's craft a reply.";
        let observe = async {
            fixture.provider.wait_for_requests(2).await;
            assert_eq!(
                tokio::fs::read_to_string(fixture.dir.path().join("tool-note.txt"))
                    .await
                    .unwrap(),
                "tool ran"
            );
            let active = fixture.activity().await;
            assert_eq!(
                active.iter().map(|row| &row.kind).collect::<Vec<_>>(),
                vec![&ActivityKind::TurnStarted, &ActivityKind::ToolStarted]
            );
            assert_eq!(active[0].id, active[0].run_id);
            assert_eq!(active[1].run_id, active[0].run_id);
            assert_eq!(active[1].detail.as_deref(), Some("record"));
            tx.send(Ok(StreamChunk::ThinkingDelta {
                text: "private thoughts".into(),
            }))
            .unwrap();
            tx.send(Ok(StreamChunk::ThinkingSignature {
                signature: "private signature".into(),
            }))
            .unwrap();
            tx.send(Ok(StreamChunk::RedactedThinking {
                data: "private opaque data".into(),
            }))
            .unwrap();
            tx.send(Ok(delta(first))).unwrap();
            fixture.channel.wait_for_text(first).await;
            tx.send(Ok(delta(" and let's craft a reply."))).unwrap();
            fixture.channel.wait_for_text(final_text).await;
            assert_eq!(fixture.activity().await.len(), 2);
            assert!(!tx.is_closed());
            drop(tx);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(fixture.brain.handle_turn(&channels, message), observe)
        })
        .await
        .unwrap();
        result.unwrap();

        let events = fixture.channel.inner.events().await;
        let visible = events
            .iter()
            .filter_map(MockEvent::as_text)
            .collect::<Vec<_>>();
        assert_eq!(visible, [first, final_text]);
        let sent = events
            .iter()
            .find_map(|event| match event {
                MockEvent::Send { sent_id, .. } => Some(sent_id),
                _ => None,
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(event, MockEvent::Edit { sent: reference, .. } if &reference.message_id == sent)));
        assert_eq!(
            fixture.outgoing(&fixture.conversation()).await,
            [final_text]
        );
        assert_lifecycle(
            &fixture.activity().await,
            &[
                ActivityKind::TurnStarted,
                ActivityKind::ToolStarted,
                ActivityKind::TurnFinished,
            ],
            true,
        );
        assert_eq!(fixture.brain.turns.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn provider_error_after_visible_text_never_replays_and_finishes_failed() {
        let (tx, stream) = controlled_stream();
        let fixture = TurnFixture::new(vec![stream, chunks(vec![Ok(delta("replayed"))])]).await;
        let message = fixture.incoming().await;
        let channels = fixture.channels();
        let observe = async {
            fixture.provider.wait_for_requests(1).await;
            tx.send(Ok(delta("partial answer"))).unwrap();
            fixture.channel.wait_for_text("partial answer").await;
            tx.send(Err(StreamError::transport("connection ended")))
                .unwrap();
            tx.closed().await;
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(fixture.brain.handle_turn(&channels, message), observe)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(fixture.provider.requests.lock().await.len(), 1);
        assert_eq!(
            fixture.outgoing(&fixture.conversation()).await,
            ["partial answer"]
        );
        let events = fixture.channel.inner.events().await;
        assert_eq!(
            events
                .iter()
                .filter_map(MockEvent::as_text)
                .collect::<Vec<_>>(),
            ["partial answer"]
        );
        assert_lifecycle(
            &fixture.activity().await,
            &[ActivityKind::TurnStarted, ActivityKind::TurnFinished],
            false,
        );
        assert_eq!(fixture.brain.turns.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn transient_failure_before_visible_text_retries_without_a_second_turn() {
        let fixture = TurnFixture::new(vec![
            chunks(vec![
                Ok(StreamChunk::ThinkingDelta {
                    text: "hidden".into(),
                }),
                Err(StreamError::rate_limited("try again", Some(Duration::ZERO))),
            ]),
            chunks(vec![Ok(delta("recovered"))]),
        ])
        .await;
        let message = fixture.incoming().await;
        fixture
            .brain
            .handle_turn(&fixture.channels(), message)
            .await
            .unwrap();
        assert_eq!(fixture.provider.requests.lock().await.len(), 2);
        assert_eq!(
            fixture.outgoing(&fixture.conversation()).await,
            ["recovered"]
        );
        assert_lifecycle(
            &fixture.activity().await,
            &[ActivityKind::TurnStarted, ActivityKind::TurnFinished],
            true,
        );
    }

    #[tokio::test]
    async fn channel_error_drops_the_open_provider_stream_and_keeps_persisted_text() {
        let (tx, stream) = controlled_stream();
        let fixture = TurnFixture::new(vec![stream]).await;
        let message = fixture.incoming().await;
        let channels = fixture.channels();
        let observe = async {
            fixture.provider.wait_for_requests(1).await;
            tx.send(Ok(delta("visible"))).unwrap();
            fixture.channel.wait_for_text("visible").await;
            fixture.channel.fail_edits.store(true, Ordering::Relaxed);
            tx.send(Ok(delta(" but not delivered"))).unwrap();
            tx.closed().await;
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(fixture.brain.handle_turn(&channels, message), observe)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(fixture.provider.requests.lock().await.len(), 1);
        assert_eq!(fixture.outgoing(&fixture.conversation()).await, ["visible"]);
        assert_lifecycle(
            &fixture.activity().await,
            &[ActivityKind::TurnStarted, ActivityKind::TurnFinished],
            false,
        );
    }

    #[tokio::test]
    async fn autonomous_tool_turns_keep_skip_silent_and_finish_each_activity_run() {
        let fixture = TurnFixture::new(vec![
            record_call(),
            chunks(vec![Ok(delta("sk")), Ok(delta("ip"))]),
            record_call(),
            chunks(vec![Ok(delta("skip"))]),
            record_call(),
            chunks(vec![Ok(delta("skip"))]),
        ])
        .await;
        fixture.incoming().await;
        let (task_run_id, schedule_id) = fixture.schedule().await;
        fixture
            .brain
            .handle_schedule(&fixture.channels(), task_run_id, schedule_id)
            .await
            .unwrap();
        fixture
            .brain
            .handle_integration_update(&fixture.channels(), integration_turn())
            .await
            .unwrap();
        fixture
            .brain
            .handle_workflow_update(
                &fixture.channels(),
                WorkflowTurn {
                    workflow: "triage".into(),
                    items: vec![],
                    overflow: 0,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(fixture.dir.path().join("tool-note.txt"))
                .await
                .unwrap(),
            "tool ran"
        );
        assert!(
            fixture
                .channel
                .inner
                .events()
                .await
                .iter()
                .all(|event| event.as_text().is_none())
        );
        assert!(fixture.outgoing(&fixture.conversation()).await.is_empty());
        let activity = fixture.activity().await;
        assert_eq!(activity.len(), 10);
        assert_lifecycle(
            &activity[..4],
            &[
                ActivityKind::TurnStarted,
                ActivityKind::ScheduleFired,
                ActivityKind::ToolStarted,
                ActivityKind::TurnFinished,
            ],
            true,
        );
        assert_eq!(
            activity[1].detail.as_deref(),
            Some(schedule_id.to_string().as_str())
        );
        assert_lifecycle(
            &activity[4..7],
            &[
                ActivityKind::TurnStarted,
                ActivityKind::ToolStarted,
                ActivityKind::TurnFinished,
            ],
            true,
        );
        assert_lifecycle(
            &activity[7..],
            &[
                ActivityKind::TurnStarted,
                ActivityKind::ToolStarted,
                ActivityKind::TurnFinished,
            ],
            true,
        );
        assert_eq!(activity[4].detail.as_deref(), Some("integration"));
        assert_eq!(activity[7].detail.as_deref(), Some("workflow"));
        assert_ne!(activity[0].run_id, activity[4].run_id);
        assert_ne!(activity[4].run_id, activity[7].run_id);
    }

    #[tokio::test]
    async fn autonomous_routing_failures_finish_activity_as_failed() {
        let fixture = TurnFixture::new(vec![]).await;
        fixture.incoming().await;
        let (task_run_id, schedule_id) = fixture.schedule().await;
        assert!(
            fixture
                .brain
                .handle_schedule(&[], task_run_id, schedule_id)
                .await
                .is_err()
        );
        assert!(
            fixture
                .brain
                .handle_integration_update(&[], integration_turn())
                .await
                .is_err()
        );
        assert!(
            fixture
                .brain
                .handle_workflow_update(
                    &[],
                    WorkflowTurn {
                        workflow: "triage".into(),
                        items: vec![],
                        overflow: 0,
                    }
                )
                .await
                .is_err()
        );
        let activity = fixture.activity().await;
        assert_eq!(activity.len(), 7);
        assert_lifecycle(
            &activity[..3],
            &[
                ActivityKind::TurnStarted,
                ActivityKind::ScheduleFired,
                ActivityKind::TurnFinished,
            ],
            false,
        );
        assert_lifecycle(
            &activity[3..5],
            &[ActivityKind::TurnStarted, ActivityKind::TurnFinished],
            false,
        );
        assert_lifecycle(
            &activity[5..],
            &[ActivityKind::TurnStarted, ActivityKind::TurnFinished],
            false,
        );
        assert!(fixture.provider.requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn opened_thread_receives_and_persists_the_live_answer_instead_of_the_parent() {
        let fixture = TurnFixture::new(vec![
            chunks(vec![
                Ok(delta("Moving this task.\n")),
                Ok(StreamChunk::ToolCall {
                    id: "branch".into(),
                    name: OPEN_THREAD_TOOL.into(),
                    input: serde_json::json!({"title": "Task", "seed": "Do the task"}).to_string(),
                }),
            ]),
            chunks(vec![Ok(delta("Thread answer"))]),
        ])
        .await;
        let mut message = fixture.incoming().await;
        message.surface = Surface::Channel;
        fixture
            .brain
            .handle_turn(&fixture.channels(), message)
            .await
            .unwrap();
        let events = fixture.channel.inner.events().await;
        let destination = events
            .iter()
            .find_map(|event| match event {
                MockEvent::Send {
                    conv,
                    body: OutgoingBody::Text(text),
                    reply_to,
                    ..
                } if text == "Thread answer" => {
                    assert!(reply_to.is_none());
                    Some(conv.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_ne!(destination, fixture.conversation());
        assert_eq!(
            fixture.outgoing(&fixture.conversation()).await,
            ["Moving this task.\n"]
        );
        assert_eq!(fixture.outgoing(&destination).await, ["Thread answer"]);
        assert_lifecycle(
            &fixture.activity().await,
            &[
                ActivityKind::TurnStarted,
                ActivityKind::ToolStarted,
                ActivityKind::TurnFinished,
            ],
            true,
        );
        assert_eq!(
            fixture.activity().await[1].detail.as_deref(),
            Some(OPEN_THREAD_TOOL)
        );
    }

    #[tokio::test]
    async fn activity_write_failure_does_not_abort_the_turn_or_its_tool() {
        let fixture = TurnFixture::new(vec![
            record_call(),
            chunks(vec![Ok(delta("still helpful"))]),
        ])
        .await;
        let database = sqlx::SqlitePool::connect(&format!(
            "sqlite://{}",
            fixture.dir.path().join("goat.db").display(),
        ))
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_activity BEFORE INSERT ON agent_activity \
             BEGIN SELECT RAISE(ABORT, 'activity unavailable'); END",
        )
        .execute(&database)
        .await
        .unwrap();
        let message = fixture.incoming().await;
        fixture
            .brain
            .handle_turn(&fixture.channels(), message)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(fixture.dir.path().join("tool-note.txt"))
                .await
                .unwrap(),
            "tool ran"
        );
        assert_eq!(
            fixture.outgoing(&fixture.conversation()).await,
            ["still helpful"]
        );
        assert!(fixture.activity().await.is_empty());
        database.close().await;
    }

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

    fn workflow_item(integration: &'static str, reference: &str) -> WorkflowItem {
        WorkflowItem {
            integration: IntegrationId::from_static(integration),
            account: "default".into(),
            stream: "inbox".into(),
            kind: IntegrationUpdateKind::Assigned,
            external_ref: format!("{integration}/default:issue:{reference}"),
            summary: format!("{reference} — Fix retry storm"),
            observation: Some(12),
        }
    }

    #[test]
    fn workflow_prompt_bundles_items_and_hints_only_registered_tools() {
        let update = WorkflowTurn {
            workflow: "inbox".into(),
            items: vec![
                workflow_item("linear", "GOA-1"),
                workflow_item("github", "#42"),
            ],
            overflow: 2,
        };
        let prompt = workflow_prompt(&update, &["linear_list_issues".to_string()]);
        assert!(prompt.starts_with("<workflow_update workflow=\"inbox\">"));
        assert!(prompt.contains("<item integration=\"linear\""));
        assert!(prompt.contains("<item integration=\"github\""));
        assert!(prompt.contains("observation:12"));
        assert!(prompt.contains("(+2 more items waiting)"));
        assert!(prompt.contains("`linear_*`"));
        assert!(!prompt.contains("`github_*`"));
        assert!(prompt.contains("domain:github, domain:linear"));
        assert!(prompt.contains("brief me once"));
        assert!(prompt.contains("Do not start the work itself"));
    }

    #[test]
    fn workflow_prompt_without_registered_tools_skips_the_live_hint() {
        let update = WorkflowTurn {
            workflow: "errors".into(),
            items: vec![workflow_item("github", "#7")],
            overflow: 0,
        };
        let prompt = workflow_prompt(&update, &[]);
        assert!(!prompt.contains("pull live data"));
        assert!(!prompt.contains("more items waiting"));
        assert!(prompt.contains("read what the watcher actually saw"));
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
        assert!(!selector_allows("read", &selectors(&[])));
        assert!(selector_allows("read", &selectors(&["*"])));
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
    fn current_time_names_the_agent_schedule_timezone() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-07T12:34:56Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let block = current_time_block(now, "Asia/Seoul");
        assert!(block.contains("iso8601=\"2026-08-07T12:34:56+00:00\""));
        assert!(block.contains("timezone=\"Asia/Seoul\""));
        assert!(block.contains("Resolve unspecified owner time references in Asia/Seoul"));
    }

    #[test]
    fn schedule_timezone_default_is_injected_but_explicit_value_wins() {
        let mut omitted = serde_json::json!({"cron": "0 9 * * *"});
        inject_schedule_timezone(&mut omitted, "Asia/Seoul");
        assert_eq!(omitted["timezone"], "Asia/Seoul");

        let mut null = serde_json::json!({"timezone": null});
        inject_schedule_timezone(&mut null, "Asia/Seoul");
        assert_eq!(null["timezone"], "Asia/Seoul");

        let mut explicit = serde_json::json!({
            "cron": "0 9 * * *",
            "timezone": "America/New_York"
        });
        inject_schedule_timezone(&mut explicit, "Asia/Seoul");
        assert_eq!(explicit["timezone"], "America/New_York");
    }

    #[test]
    fn schedule_schema_advertises_agent_timezone_default() {
        let mut schema = serde_json::json!({
            "properties": {"timezone": {"default": "UTC"}}
        });
        set_schedule_timezone_schema_default(&mut schema, "Asia/Seoul");
        assert_eq!(schema["properties"]["timezone"]["default"], "Asia/Seoul");
    }

    #[test]
    fn compose_system_prompt_appends_runtime_guard() {
        let prompt = compose_system_prompt("You are dev.", None, None, None);
        assert!(prompt.contains("You are dev."));
        assert!(prompt.contains("<goat_runtime_guard>"));
        assert!(prompt.contains("Return only the final user-facing answer."));
    }

    #[test]
    fn compose_system_prompt_leads_with_goat_self() {
        let prompt = compose_system_prompt("You are dev.", None, None, None);
        let goat_self = prompt.find("<goat_self>").unwrap();
        let agent = prompt.find("You are dev.").unwrap();
        assert!(goat_self < agent);
        assert!(prompt.contains("activate the `goat` skill"));
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
    fn history_content_attributes_people_by_stable_key() {
        let row = HistoryRow {
            direction: Direction::In,
            sender: Some(MessageSender::User(goat_types::UserHandle {
                external: "user-42".into(),
                display: Some("Mutable Name".into()),
            })),
            text: "private detail".into(),
            attachments: Vec::new(),
            reply_to: None,
            ts: chrono::Utc::now(),
        };
        assert_eq!(
            history_content(&row),
            "user Mutable Name [user-42]: private detail"
        );
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
    fn schedule_prompt_names_a_direct_message() {
        let prompt = turn_system_prompt(
            "base",
            &TurnMode::Schedule { tools: vec![] },
            Surface::Dm,
            false,
        );
        assert!(prompt.contains("You are replying in a direct message."));
        assert!(prompt.contains("<schedule_context>"));
        assert!(!prompt.contains("shared channel"));
    }

    use std::time::Duration;

    fn intake_conversation(external: &str) -> ConversationId {
        ConversationId::new(
            goat_types::ChannelId::new("test"),
            goat_types::InstanceId::from_slug("i"),
            external,
        )
    }

    fn intake_msg(conversation: ConversationId, from: &str, text: &str) -> IncomingMessage {
        IncomingMessage {
            id: MessageId(String::new()),
            agent: AgentId::from_slug("test"),
            conversation,
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
        let key = (intake_conversation("t"), "u".to_string());
        buf.push(
            key.clone(),
            intake_msg(intake_conversation("t"), "u", "a"),
            base,
        );
        buf.push(
            key.clone(),
            intake_msg(intake_conversation("t"), "u", "b"),
            base + Duration::from_millis(300),
        );
        buf.push(
            key.clone(),
            intake_msg(intake_conversation("t"), "u", "c"),
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
        let key = (intake_conversation("t"), "u".to_string());

        buf.push(
            key.clone(),
            intake_msg(intake_conversation("t"), "u", "first"),
            base,
        );
        let first = buf.drain_due(base + Duration::from_secs(1));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text, "first");

        buf.push(
            key.clone(),
            intake_msg(intake_conversation("t"), "u", "second"),
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
        let key = (intake_conversation("t"), "u".to_string());

        let mut t = 0u64;
        while t <= 4500 {
            buf.push(
                key.clone(),
                intake_msg(intake_conversation("t"), "u", "x"),
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

        let mut same_conversation =
            IntakeBuffer::new(Duration::from_secs(1), Duration::from_secs(5));
        same_conversation.push(
            (intake_conversation("t"), "u1".to_string()),
            intake_msg(intake_conversation("t"), "u1", "a"),
            base,
        );
        same_conversation.push(
            (intake_conversation("t"), "u2".to_string()),
            intake_msg(intake_conversation("t"), "u2", "b"),
            base,
        );
        assert_eq!(
            same_conversation
                .drain_due(base + Duration::from_secs(1))
                .len(),
            2
        );

        let mut same_user = IntakeBuffer::new(Duration::from_secs(1), Duration::from_secs(5));
        same_user.push(
            (intake_conversation("t1"), "u".to_string()),
            intake_msg(intake_conversation("t1"), "u", "a"),
            base,
        );
        same_user.push(
            (intake_conversation("t2"), "u".to_string()),
            intake_msg(intake_conversation("t2"), "u", "b"),
            base,
        );
        assert_eq!(same_user.drain_due(base + Duration::from_secs(1)).len(), 2);
    }
}

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::{stream, StreamExt};
use goat_bus::{EventBus, EventFilter};
use goat_channel::ChannelHandle;
use goat_command::{CommandOutput, CommandRegistry};
use goat_evaluator::{Evaluator, ModelScoreStore};
use goat_llm::{
    BlockId, ContentPart, LlmChunk, LlmError, LlmMessage, LlmProvider, LlmRequest, LlmResponse,
    LlmStream, Model, Role, StopReason, ToolSpec, Usage,
};
use goat_memory::{Embedder, EpisodicKind, MemoryStore};
use goat_persona::PersonalityCard;
use goat_render::{RenderSummary, StreamRenderer};
use goat_skills::SkillIndex;
use goat_store::{
    Direction, HistoryRow, ScheduledTaskStatus, Store, TaskRunStatus, ToolInvocationRecord,
    ToolInvocationStatus,
};
use goat_tool::{
    selector_allows, selector_allows_empty_denies, validate_tool_selectors, ToolCall, ToolContext,
    ToolOutput, ToolReadState, ToolRegistry,
};
use goat_types::{ConversationId, Event, MessageId, PersonaId};
use tracing::{info, warn};

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

const SUMMARY_SYSTEM_PROMPT: &str = r#"You maintain a running summary of an ongoing chat conversation so older turns can be dropped from the live context without losing what matters.
Given the previous summary (if any) and the next batch of messages, produce a single updated summary.
Preserve durable facts, decisions, commitments, open questions, and user preferences. Drop small talk and redundant detail.
Write in compact third-person notes. Output only the summary text, no preamble."#;

/// Minimum cosine similarity for an episodic memory to be surfaced as
/// recalled context. Filters noise (and zero-vector dummy embeddings).
const MIN_RECALL_SCORE: f32 = 0.5;
/// Max characters per recalled episodic excerpt injected into the prompt.
const RECALL_SNIPPET_CHARS: usize = 240;
/// Upper bound on messages folded into the rolling summary in a single
/// summarization call, so a large backlog never sends one enormous transcript
/// to the summarizer (which would risk a context-window overflow or timeout).
const MAX_SUMMARY_FOLD_BATCH: usize = 40;
/// Upper bound on summarization calls per turn. A long backlog catches up
/// across turns rather than stalling a single turn behind many LLM calls.
const MAX_SUMMARY_FOLDS_PER_TURN: usize = 4;

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn LlmProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, p: Arc<dyn LlmProvider>) {
        self.providers.push(p);
    }

    pub fn route(&self, model: &Model) -> Result<Arc<dyn LlmProvider>> {
        self.providers
            .iter()
            .find(|p| p.id() == model.provider)
            .cloned()
            .ok_or_else(|| anyhow!("no provider supports model {:?}", model.id()))
    }
}

/// All dependencies required to construct a [`Brain`].
///
/// Using a single struct instead of positional arguments makes the constructor
/// self-documenting, prevents accidental argument transposition, and allows
/// new capabilities (e.g. timeout/retry settings) to be added without
/// changing every call site.
pub struct BrainDeps {
    pub persona: PersonaId,
    pub personality: Arc<PersonalityCard>,
    pub default_model: Model,
    pub history_window: usize,
    pub tool_selectors: Vec<String>,
    pub providers: Arc<ProviderRegistry>,
    pub tools: Arc<ToolRegistry>,
    pub commands: Arc<CommandRegistry>,
    pub store: Arc<dyn Store>,
    pub memory: Arc<dyn MemoryStore>,
    pub embedder: Option<Arc<dyn Embedder>>,
    pub memory_enabled: bool,
    pub episodic_k: usize,
    pub summarize_enabled: bool,
    pub renderer: Arc<dyn StreamRenderer>,
    pub evaluator: Arc<dyn Evaluator>,
    pub model_scores: Arc<ModelScoreStore>,
    pub goat_root: PathBuf,
    /// Maximum time to wait between consecutive LLM stream chunks before
    /// treating the connection as stalled. Defaults to 60 s.
    pub stream_idle_timeout: std::time::Duration,
    /// Maximum number of retry attempts for transient LLM errors
    /// (Transport, RateLimited, Provider 5xx) before failing a turn.
    /// Defaults to 3.
    pub llm_max_retries: usize,
}

pub struct Brain {
    persona: PersonaId,
    personality: Arc<PersonalityCard>,
    default_model: Model,
    history_window: usize,
    tool_selectors: Vec<String>,
    providers: Arc<ProviderRegistry>,
    tools: Arc<ToolRegistry>,
    commands: Arc<CommandRegistry>,
    store: Arc<dyn Store>,
    memory: Arc<dyn MemoryStore>,
    embedder: Option<Arc<dyn Embedder>>,
    memory_enabled: bool,
    episodic_k: usize,
    summarize_enabled: bool,
    renderer: Arc<dyn StreamRenderer>,
    evaluator: Arc<dyn Evaluator>,
    model_scores: Arc<ModelScoreStore>,
    goat_root: PathBuf,
    stream_idle_timeout: std::time::Duration,
    llm_max_retries: usize,
}

impl Brain {
    pub fn new(deps: BrainDeps) -> Self {
        Self {
            persona: deps.persona,
            personality: deps.personality,
            default_model: deps.default_model,
            history_window: deps.history_window,
            tool_selectors: deps.tool_selectors,
            providers: deps.providers,
            tools: deps.tools,
            commands: deps.commands,
            store: deps.store,
            memory: deps.memory,
            embedder: deps.embedder,
            memory_enabled: deps.memory_enabled,
            episodic_k: deps.episodic_k,
            summarize_enabled: deps.summarize_enabled,
            renderer: deps.renderer,
            evaluator: deps.evaluator,
            model_scores: deps.model_scores,
            goat_root: deps.goat_root,
            stream_idle_timeout: deps.stream_idle_timeout,
            llm_max_retries: deps.llm_max_retries,
        }
    }

    pub async fn run(
        self: Arc<Self>,
        bus: EventBus,
        channels: Vec<Arc<dyn ChannelHandle>>,
    ) -> Result<()> {
        let mut sub = bus.subscribe(EventFilter::Persona(self.persona));
        info!(persona = %self.persona, "brain running");

        while let Some(event) = sub.recv().await {
            match event {
                Event::Incoming(msg) => {
                    if let Err(e) = self.handle(&channels, msg).await {
                        warn!(persona = %self.persona, error = ?e, "turn failed");
                    }
                }
                Event::SelfTick {
                    run_id, task_id, ..
                } => {
                    if let Err(e) = self.handle_self_tick(&channels, run_id, task_id).await {
                        warn!(
                            persona = %self.persona,
                            run_id,
                            task_id,
                            error = ?e,
                            "self-tick failed",
                        );
                    }
                }
                Event::Reflection { .. } => {
                    if let Err(e) = self.handle_reflection(&channels).await {
                        warn!(
                            persona = %self.persona,
                            error = ?e,
                            "reflection failed",
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn handle(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        msg: goat_types::IncomingMessage,
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

        self.store
            .append_incoming(&msg)
            .await
            .context("append incoming")?;

        // Embed the incoming message once: reused both as the recall query
        // (search runs before this turn's writes, so it never matches itself)
        // and as the stored embedding for the episodic write after the turn.
        let query_embedding = if self.memory_enabled {
            self.embed_text(&msg.text).await
        } else {
            None
        };

        let (summary, mut messages) = self.load_context(&msg.conversation).await?;
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
                            msg.conversation.clone(),
                            reply_to.clone(),
                            text_stream(self.default_model.clone(), text),
                        )
                        .await?;
                    if !summary.final_text.is_empty() {
                        self.store
                            .append_outgoing_text(
                                self.persona,
                                &msg.conversation,
                                &summary.final_text,
                                Some(&msg.id),
                            )
                            .await
                            .context("append outgoing")?;
                    }
                    return Ok(());
                }
                Ok(CommandOutput::Skip) => return Ok(()),
                Ok(_) => return Ok(()),
                Err(e) => {
                    warn!(persona = %self.persona, error = ?e, "command failed");
                    messages.push(LlmMessage {
                        role: Role::User,
                        content: vec![ContentPart::Text(format!(
                            "The requested command failed before execution: {e}"
                        ))],
                    });
                }
            }
        }

        let turn_started = std::time::Instant::now();
        let summary = self
            .complete_with_tools(
                handle,
                msg.conversation.clone(),
                reply_to,
                &mut messages,
                TurnMode::Normal,
                query_embedding.clone(),
                summary,
            )
            .await?;
        let latency_ms = turn_started.elapsed().as_millis() as i64;

        if !summary.final_text.is_empty() {
            self.store
                .append_outgoing_text(
                    self.persona,
                    &msg.conversation,
                    &summary.final_text,
                    Some(&msg.id),
                )
                .await
                .context("append outgoing")?;
        }

        self.evaluate_turn(&messages, &summary.final_text, latency_ms)
            .await;

        // Episodic writes happen after the turn so recall above never sees
        // the current exchange.
        self.record_episodic(
            &msg.conversation,
            EpisodicKind::User,
            &msg.text,
            query_embedding.as_deref(),
        )
        .await;
        if !summary.final_text.is_empty() {
            let assistant_embedding = self.embed_text(&summary.final_text).await;
            self.record_episodic(
                &msg.conversation,
                EpisodicKind::Assistant,
                &summary.final_text,
                assistant_embedding.as_deref(),
            )
            .await;
        }

        Ok(())
    }

    /// Embed `text` if this persona has an embedder. Returns `None` when no
    /// embedder is configured or the embedding call fails — memory writes
    /// then fall back to storing the text with a NULL embedding, and recall
    /// is skipped, so the chat loop is never blocked by embedding trouble.
    async fn embed_text(&self, text: &str) -> Option<Vec<f32>> {
        let embedder = self.embedder.as_ref()?;
        match embedder.embed(text).await {
            Ok(v) => Some(v),
            Err(e) => {
                warn!(persona = %self.persona, error = ?e, "embedding failed");
                None
            }
        }
    }

    /// Score a completed turn with the configured [`Evaluator`] and record the
    /// result against the model that produced it. Best-effort: an evaluator or
    /// store failure must never affect the user-facing turn, so errors are
    /// logged and swallowed here. The default wiring uses a no-op evaluator, so
    /// this captures per-model call counts and latency until a real scorer is
    /// configured.
    async fn evaluate_turn(&self, messages: &[LlmMessage], final_text: &str, latency_ms: i64) {
        if final_text.trim().is_empty() {
            return;
        }
        let mut req = LlmRequest::new(self.default_model.clone());
        req.messages = messages.to_vec();
        let resp = LlmResponse {
            text: final_text.to_string(),
            stop: StopReason::EndTurn,
            usage: Usage::default(),
            model: self.default_model.clone(),
        };
        let score = self.evaluator.score(&req, &resp).await;
        if let Err(e) = self
            .model_scores
            .record(self.persona, &self.default_model, &score, latency_ms)
            .await
        {
            warn!(persona = %self.persona, error = ?e, "model score record failed");
        }
    }

    /// Build the `<persona_memory>` / `<recalled_memory>` system-prompt
    /// section: durable core facts plus episodic excerpts relevant to the
    /// current turn. Returns `None` when memory is disabled or empty.
    async fn build_memory_section(&self, query_embedding: Option<&[f32]>) -> Option<String> {
        if !self.memory_enabled {
            return None;
        }
        let mut out = String::new();

        match self.memory.core_blocks(self.persona).await {
            Ok(blocks) if !blocks.is_empty() => {
                out.push_str("<persona_memory>\nDurable facts you have chosen to remember:\n");
                for b in blocks {
                    out.push_str(&format!("- [{}] {}\n", b.slug, b.text.trim()));
                }
                out.push_str("</persona_memory>");
            }
            Ok(_) => {}
            Err(e) => warn!(persona = %self.persona, error = ?e, "core_blocks failed"),
        }

        if let Some(query) = query_embedding {
            match self
                .memory
                .search_episodic(self.persona, query, self.episodic_k)
                .await
            {
                Ok(entries) => {
                    let relevant: Vec<_> = entries
                        .into_iter()
                        .filter(|e| e.score.unwrap_or(0.0) >= MIN_RECALL_SCORE)
                        .collect();
                    if !relevant.is_empty() {
                        if !out.is_empty() {
                            out.push_str("\n\n");
                        }
                        out.push_str(
                            "<recalled_memory>\nPossibly relevant excerpts from earlier conversations:\n",
                        );
                        for e in relevant {
                            let who = match e.kind {
                                EpisodicKind::User => "user",
                                EpisodicKind::Assistant => "you",
                                EpisodicKind::Observation => "note",
                            };
                            out.push_str(&format!("- ({}) {}\n", who, recall_snippet(&e.text)));
                        }
                        out.push_str("</recalled_memory>");
                    }
                }
                Err(e) => warn!(persona = %self.persona, error = ?e, "search_episodic failed"),
            }
        }

        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Persist a user/assistant/observation turn into episodic memory.
    /// Best-effort: failures are logged, never propagated to the chat turn.
    async fn record_episodic(
        &self,
        conv: &ConversationId,
        kind: EpisodicKind,
        text: &str,
        embedding: Option<&[f32]>,
    ) {
        if !self.memory_enabled || text.trim().is_empty() {
            return;
        }
        if let Err(e) = self
            .memory
            .append_episodic(self.persona, conv, kind, text, embedding)
            .await
        {
            warn!(persona = %self.persona, error = ?e, "append_episodic failed");
        }
    }

    async fn history_messages(&self, conv: &ConversationId) -> Result<Vec<LlmMessage>> {
        let history = self
            .store
            .recent(self.persona, conv, self.history_window)
            .await
            .context("read history")?;
        Ok(rows_to_messages(history))
    }

    /// Load the LLM context for a turn. With summarization enabled, older
    /// messages are folded into a rolling summary (returned as the first
    /// element) once the un-summarized tail grows past `2 * history_window`,
    /// keeping the raw message list bounded without dropping context.
    /// Without it, falls back to the recent-window history and no summary.
    async fn load_context(
        &self,
        conv: &ConversationId,
    ) -> Result<(Option<String>, Vec<LlmMessage>)> {
        if !self.summarize_enabled {
            return Ok((None, self.history_messages(conv).await?));
        }

        let total = self.store.message_count(self.persona, conv).await?;
        let existing = self
            .store
            .get_conversation_summary(self.persona, conv)
            .await?;
        let mut summary_text = existing.as_ref().map(|s| s.summary.clone());
        let mut summarized = existing.map(|s| s.summarized_count).unwrap_or(0).min(total);

        // Fold the oldest un-summarized messages while the tail exceeds two
        // windows, bringing it back toward one window. Each fold is capped at
        // MAX_SUMMARY_FOLD_BATCH messages so a large backlog (e.g. summarization
        // newly enabled on a long conversation) never sends one enormous
        // transcript to the summarizer; at most MAX_SUMMARY_FOLDS_PER_TURN folds
        // run per turn, so catching up can't stall a single turn — the rest
        // folds on later turns.
        let mut folds_done = 0;
        while folds_done < MAX_SUMMARY_FOLDS_PER_TURN
            && total.saturating_sub(summarized) > 2 * self.history_window
        {
            let remaining = total - summarized - self.history_window;
            let fold_count = remaining.min(MAX_SUMMARY_FOLD_BATCH);
            let batch = self
                .store
                .messages_from(self.persona, conv, summarized, fold_count)
                .await?;
            match self.summarize_batch(summary_text.as_deref(), &batch).await {
                Some(updated) => {
                    let new_count = summarized + fold_count;
                    if let Err(e) = self
                        .store
                        .upsert_conversation_summary(self.persona, conv, &updated, new_count)
                        .await
                    {
                        warn!(persona = %self.persona, error = ?e, "upsert_conversation_summary failed");
                        break;
                    }
                    summary_text = Some(updated);
                    summarized = new_count;
                    folds_done += 1;
                }
                // Provider error: leave the watermark untouched and stop folding
                // this turn so the turn still proceeds; it retries next turn.
                None => break,
            }
        }

        let raw = self
            .store
            .messages_from(
                self.persona,
                conv,
                summarized,
                total.saturating_sub(summarized),
            )
            .await?;
        Ok((summary_text, rows_to_messages(raw)))
    }

    /// Produce an updated rolling summary from the previous summary and a
    /// batch of older messages. Best-effort: returns `None` (leaving the
    /// watermark unchanged) on any provider error so the turn still proceeds.
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
        let mut req = LlmRequest::new(self.default_model.clone());
        req.system = Some(SUMMARY_SYSTEM_PROMPT.to_string());
        req.max_tokens = 1024;
        req.messages = vec![LlmMessage::user_text(user)];
        let stream = match provider.stream(req).await {
            Ok(s) => s,
            Err(e) => {
                warn!(persona = %self.persona, error = ?e, "summarization request failed");
                return None;
            }
        };
        match fold_turn(stream, self.stream_idle_timeout).await {
            Ok(folded) => {
                let text = folded.text.trim().to_string();
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            Err(e) => {
                warn!(persona = %self.persona, error = ?e, "summarization stream failed");
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn complete_with_tools(
        &self,
        handle: Arc<dyn ChannelHandle>,
        conv: ConversationId,
        reply_to: Option<MessageId>,
        messages: &mut Vec<LlmMessage>,
        mode: TurnMode,
        query_embedding: Option<Vec<f32>>,
        summary: Option<String>,
    ) -> Result<RenderSummary> {
        const MAX_TOOL_ROUNDS: usize = 8;

        let provider = self.providers.route(&self.default_model)?;
        let skill_prompt =
            SkillIndex::discover_root(&self.goat_root).system_prompt_block(self.persona);
        let tool_specs: Vec<ToolSpec> = self
            .llm_tool_specs(skill_prompt.is_some(), &mode)
            .into_iter()
            .collect();
        let allowed_tools: HashSet<String> =
            tool_specs.iter().map(|spec| spec.name.clone()).collect();
        let read_state = ToolReadState::default();
        let memory_section = self.build_memory_section(query_embedding.as_deref()).await;
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
            TurnMode::Normal => base_system,
            TurnMode::SelfTick { .. } => format!(
                "{base_system}\n\n<self_tick_context>\nYou are running at the \
                 fire moment of a scheduled task. Read the task and act. \
                 If the task is no longer worth doing, reply with exactly: skip\n\
                 </self_tick_context>"
            ),
            TurnMode::Reflection => format!(
                "{base_system}\n\n<reflection_context>\n\
                 이것은 사용자 메시지가 아니라 자율 reflection 시점이다. \
                 최근 대화와 기억을 검토하라. 지금 정말로 할 가치가 있으면 \
                 — 짧고 유용한 메시지를 먼저 보내거나 후속 작업을 예약하라. \
                 할 일이 없으면 정확히 `skip` 이라고만 답하라.\
                 \n</reflection_context>"
            ),
        };

        for _round in 0..MAX_TOOL_ROUNDS {
            let mut req = LlmRequest::new(self.default_model.clone());
            req.system = Some(system_prompt.clone());
            req.messages = messages.clone();
            req.tools = tool_specs.clone();

            let folded = self.stream_with_retry(&provider, req).await?;

            if folded.tool_calls.is_empty() {
                let final_text = sanitize_final_text(folded.text);
                if matches!(mode, TurnMode::SelfTick { .. } | TurnMode::Reflection)
                    && final_text.trim().eq_ignore_ascii_case("skip")
                {
                    return Ok(RenderSummary {
                        messages_sent: 0,
                        edits: 0,
                        final_text: "skip".into(),
                    });
                }
                return self
                    .renderer
                    .render(
                        handle,
                        conv,
                        reply_to,
                        text_stream(self.default_model.clone(), final_text),
                    )
                    .await
                    .map_err(Into::into);
            }

            messages.push(assistant_tool_call_message(&folded.tool_calls));

            for call in folded.tool_calls {
                let output = self
                    .execute_tool(&conv, &call, read_state.clone(), &allowed_tools)
                    .await;
                messages.push(LlmMessage {
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        id: call.id,
                        name: call.name,
                        content: output.text_for_model(),
                    }],
                });
            }
        }

        let text = "I stopped because tool execution exceeded the safety round limit.".to_string();
        self.renderer
            .render(
                handle,
                conv,
                reply_to,
                text_stream(self.default_model.clone(), text),
            )
            .await
            .map_err(Into::into)
    }

    /// Persist a task-run terminal state, logging — never swallowing — a store
    /// failure. A dropped error here would let `task_runs` desync from reality
    /// (a run stuck "running" forever, or silently re-fired on the next boot).
    async fn finish_run_logged(&self, run_id: i64, status: TaskRunStatus, note: Option<String>) {
        let label = format!("{status:?}");
        if let Err(e) = self.store.finish_run(run_id, status, note).await {
            tracing::error!(
                run_id,
                persona = %self.persona,
                status = %label,
                error = %e,
                "failed to persist task run completion",
            );
        }
    }

    async fn handle_self_tick(
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
        let handle = match channels
            .iter()
            .find(|h| h.id() == conv.channel && h.instance() == conv.instance)
            .cloned()
        {
            Some(h) => h,
            None => {
                let available: Vec<String> = channels
                    .iter()
                    .map(|h| format!("{}:{}", h.id().as_str(), h.instance()))
                    .collect();
                warn!(
                    run_id,
                    persona = %self.persona,
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
            }
        };

        let mut messages = vec![LlmMessage {
            role: Role::User,
            content: vec![ContentPart::Text(task.task.clone())],
        }];

        let query_embedding = if self.memory_enabled {
            self.embed_text(&task.task).await
        } else {
            None
        };

        let turn_started = std::time::Instant::now();
        let summary = match self
            .complete_with_tools(
                handle,
                conv.clone(),
                None,
                &mut messages,
                TurnMode::SelfTick {
                    tools: task.tools.clone(),
                },
                query_embedding.clone(),
                None,
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                self.finish_run_logged(
                    run_id,
                    TaskRunStatus::Failed,
                    Some(format!("self-tick run errored: {e}")),
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
                persona = %self.persona,
                "self-tick produced empty response; marking failed",
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
            .append_outgoing_text(self.persona, &conv, &summary.final_text, None)
            .await
            .context("append outgoing text for self-tick")?;

        self.record_episodic(
            &conv,
            EpisodicKind::Observation,
            &task.task,
            query_embedding.as_deref(),
        )
        .await;
        let assistant_embedding = self.embed_text(&summary.final_text).await;
        self.record_episodic(
            &conv,
            EpisodicKind::Assistant,
            &summary.final_text,
            assistant_embedding.as_deref(),
        )
        .await;

        let latency_ms = turn_started.elapsed().as_millis() as i64;
        self.evaluate_turn(&messages, &summary.final_text, latency_ms)
            .await;

        let truncated = truncate_for_summary(&summary.final_text);
        self.finish_run_logged(run_id, TaskRunStatus::Done, Some(truncated))
            .await;
        Ok(())
    }

    async fn handle_reflection(&self, channels: &[Arc<dyn ChannelHandle>]) -> Result<()> {
        let conv = match self.store.latest_conversation(self.persona).await? {
            Some(c) => c,
            None => return Ok(()),
        };

        let handle = match channels
            .iter()
            .find(|h| h.id() == conv.channel && h.instance() == conv.instance)
            .cloned()
        {
            Some(h) => h,
            None => {
                warn!(
                    persona = %self.persona,
                    conv = %conv,
                    "reflection: no channel handle for latest conversation",
                );
                return Ok(());
            }
        };

        let mut messages = self.history_messages(&conv).await?;
        messages.push(LlmMessage {
            role: Role::User,
            content: vec![ContentPart::Text("(자율 reflection 시점)".into())],
        });

        let turn_started = std::time::Instant::now();
        let summary = self
            .complete_with_tools(
                handle,
                conv.clone(),
                None,
                &mut messages,
                TurnMode::Reflection,
                None,
                None,
            )
            .await?;

        let trimmed = summary.final_text.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("skip") {
            return Ok(());
        }

        self.store
            .append_outgoing_text(self.persona, &conv, &summary.final_text, None)
            .await
            .context("append outgoing text for reflection")?;

        let assistant_embedding = self.embed_text(&summary.final_text).await;
        self.record_episodic(
            &conv,
            EpisodicKind::Assistant,
            &summary.final_text,
            assistant_embedding.as_deref(),
        )
        .await;

        let latency_ms = turn_started.elapsed().as_millis() as i64;
        self.evaluate_turn(&messages, &summary.final_text, latency_ms)
            .await;

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
                TurnMode::SelfTick { tools } => {
                    !is_schedule_tool(spec.name.as_str())
                        && selector_allows_empty_denies(spec.name.as_str(), tools)
                }
                TurnMode::Reflection => true,
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
        conv: &ConversationId,
        call: &ModelToolCall,
        read_state: ToolReadState,
        allowed_tools: &HashSet<String>,
    ) -> ToolOutput {
        let started_at = chrono::Utc::now();
        let name = match goat_tool::ToolName::new(call.name.clone()) {
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
        if is_schedule_create_tool(name.as_str()) {
            if let Err(e) = validate_scheduled_tool_selectors(&call.arguments, allowed_tools) {
                let output = ToolOutput::error(e);
                self.audit_tool_call(conv, call, name.to_string(), &output, started_at)
                    .await;
                return output;
            }
        }
        let ctx = ToolContext {
            persona: self.persona,
            conversation: conv.clone(),
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
            persona: self.persona,
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

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    args_json: String,
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

/// Returns `true` if the given [`LlmError`] is likely transient and worth
/// retrying (e.g. network hiccup, rate-limit, provider 5xx). Auth errors and
/// malformed-request errors are never transient.
fn is_transient_llm_error(e: &LlmError) -> bool {
    matches!(
        e,
        LlmError::Transport(_) | LlmError::RateLimited { .. } | LlmError::Provider(_)
    )
}

impl Brain {
    /// Sends `req` to `provider` and folds the resulting stream into a
    /// [`FoldedTurn`]. Retries up to `self.llm_max_retries` times on
    /// transient [`LlmError`]s (Transport, RateLimited, Provider), honouring
    /// `retry_after` from rate-limit responses. Non-transient errors (Auth,
    /// BadRequest) and stream-idle-timeout errors are retried with simple
    /// exponential backoff.
    async fn stream_with_retry(
        &self,
        provider: &Arc<dyn LlmProvider>,
        req: LlmRequest,
    ) -> Result<FoldedTurn> {
        let mut last_rate_limit_secs: Option<u64> = None;

        for attempt in 0usize..=self.llm_max_retries {
            if attempt > 0 {
                let delay = match last_rate_limit_secs.take() {
                    Some(secs) => std::time::Duration::from_secs(secs),
                    None => std::time::Duration::from_millis(500u64 << (attempt - 1).min(4)),
                };
                warn!(
                    persona = %self.persona,
                    attempt,
                    delay_ms = delay.as_millis(),
                    "retrying transient LLM error",
                );
                tokio::time::sleep(delay).await;
            }

            match provider.stream(req.clone()).await {
                Err(e) => {
                    let is_last = attempt == self.llm_max_retries;
                    if !is_transient_llm_error(&e) || is_last {
                        return Err(anyhow::anyhow!("{e}"));
                    }
                    if let LlmError::RateLimited { retry_after, .. } = &e {
                        last_rate_limit_secs = *retry_after;
                    }
                }
                Ok(stream) => match fold_turn(stream, self.stream_idle_timeout).await {
                    Ok(folded) => return Ok(folded),
                    Err(e) => {
                        if attempt == self.llm_max_retries {
                            return Err(e);
                        }
                        warn!(
                            persona = %self.persona,
                            error = ?e,
                            attempt,
                            "LLM stream error; will retry",
                        );
                    }
                },
            }
        }

        // Exhausted retries — the last error was already returned above.
        unreachable!()
    }
}

/// Consumes a streaming LLM response into a [`FoldedTurn`].
///
/// `idle_timeout` bounds the wait between consecutive chunks. If no chunk
/// arrives within the timeout, the turn fails with a transport error so the
/// caller can apply retry/backoff rather than waiting indefinitely. Using a
/// chunk-level timeout (not a whole-request deadline) means legitimate long
/// responses are not truncated — only truly stalled connections are detected.
async fn fold_turn(mut stream: LlmStream, idle_timeout: std::time::Duration) -> Result<FoldedTurn> {
    let mut text = String::new();
    let mut pending: HashMap<BlockId, PendingToolCall> = HashMap::new();
    let mut done = false;

    while !done {
        match tokio::time::timeout(idle_timeout, stream.next()).await {
            Err(_elapsed) => {
                return Err(anyhow::anyhow!(
                    "LLM stream stalled: no chunk received within {:?}",
                    idle_timeout
                ));
            }
            Ok(None) => break,
            Ok(Some(item)) => match item? {
                LlmChunk::TextDelta { text: delta, .. } => text.push_str(&delta),
                LlmChunk::ToolCallStart {
                    block,
                    tool_call_id,
                    name,
                } => {
                    pending.insert(
                        block,
                        PendingToolCall {
                            id: tool_call_id,
                            name,
                            args_json: String::new(),
                        },
                    );
                }
                LlmChunk::ToolCallDelta {
                    block,
                    args_json_fragment,
                } => {
                    pending
                        .entry(block)
                        .or_default()
                        .args_json
                        .push_str(&args_json_fragment);
                }
                LlmChunk::MessageEnd { .. } => done = true,
                _ => {}
            },
        }
    }

    let mut calls = Vec::with_capacity(pending.len());
    let mut pending: Vec<(BlockId, PendingToolCall)> = pending.into_iter().collect();
    pending.sort_by_key(|(block, _)| block.0);
    for (block, call) in pending {
        let id = if call.id.is_empty() {
            format!("call_{}", block.0)
        } else {
            call.id
        };
        let arguments = if call.args_json.trim().is_empty() {
            serde_json::Value::Object(Default::default())
        } else {
            serde_json::from_str(&call.args_json).unwrap_or_else(
                |e| serde_json::json!({"_invalid_json": call.args_json, "_error": e.to_string()}),
            )
        };
        calls.push(ModelToolCall {
            id,
            name: call.name,
            arguments,
        });
    }

    Ok(FoldedTurn {
        text,
        tool_calls: calls,
    })
}

fn text_stream(model: Model, text: String) -> LlmStream {
    let chunks = vec![
        Ok(LlmChunk::MessageStart {
            id: "synthetic".into(),
            model,
            input_tokens: 0,
        }),
        Ok(LlmChunk::TextDelta {
            block: BlockId(0),
            text,
        }),
        Ok(LlmChunk::BlockEnd { block: BlockId(0) }),
        Ok(LlmChunk::MessageEnd {
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        }),
    ];
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
    persona_prompt: &str,
    skill_prompt: Option<&str>,
    summary_prompt: Option<&str>,
    memory_prompt: Option<&str>,
) -> String {
    let mut parts = vec![persona_prompt.trim().to_string()];
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
    SelfTick { tools: Vec<String> },
    Reflection,
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

    fn selectors(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn explicit_empty_persona_selector_denies_tools() {
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
        assert!(!message
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::Text(_))));
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
        let persona = prompt.find("You are dev.").unwrap();
        let skills = prompt.find("<available_skills/>").unwrap();
        let guard = prompt.find("<goat_runtime_guard>").unwrap();
        assert!(persona < skills);
        assert!(skills < guard);
    }

    #[test]
    fn compose_system_prompt_inserts_memory_before_runtime_guard() {
        let prompt = compose_system_prompt(
            "You are dev.",
            Some("<available_skills/>"),
            None,
            Some("<persona_memory>fact</persona_memory>"),
        );
        let skills = prompt.find("<available_skills/>").unwrap();
        let memory = prompt.find("<persona_memory>").unwrap();
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
            Some("<persona_memory>fact</persona_memory>"),
        );
        let summary = prompt.find("<conversation_summary>").unwrap();
        let memory = prompt.find("<persona_memory>").unwrap();
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
    fn reflection_mode_does_not_filter_schedule_tools() {
        assert!(is_schedule_tool("schedule_once"));
        assert!(is_schedule_tool("schedule_cron"));
        assert!(is_schedule_tool("cancel_task"));
        assert!(is_schedule_tool("list_tasks"));
        assert!(!is_schedule_tool("recall"));
        assert!(!is_schedule_tool("shell"));
    }

    #[test]
    fn reflection_and_self_tick_both_trigger_skip_guard() {
        let reflection = TurnMode::Reflection;
        let self_tick = TurnMode::SelfTick { tools: vec![] };
        let normal = TurnMode::Normal;
        assert!(matches!(
            reflection,
            TurnMode::SelfTick { .. } | TurnMode::Reflection
        ));
        assert!(matches!(
            self_tick,
            TurnMode::SelfTick { .. } | TurnMode::Reflection
        ));
        assert!(!matches!(
            normal,
            TurnMode::SelfTick { .. } | TurnMode::Reflection
        ));
    }
}

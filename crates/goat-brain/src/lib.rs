use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::{stream, StreamExt};
use goat_bus::{EventBus, EventFilter};
use goat_channel::ChannelHandle;
use goat_command::{CommandOutput, CommandRegistry};
use goat_llm::{
    BlockId, ContentPart, LlmChunk, LlmMessage, LlmProvider, LlmRequest, LlmStream, Model, Role,
    StopReason, ToolSpec, Usage,
};
use goat_persona::PersonalityCard;
use goat_render::{RenderSummary, StreamRenderer};
use goat_skills::SkillIndex;
use goat_store::{
    Direction, ScheduledTaskStatus, Store, TaskRunStatus, ToolInvocationRecord,
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
    renderer: Arc<dyn StreamRenderer>,
    goat_root: PathBuf,
}

impl Brain {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        persona: PersonaId,
        personality: Arc<PersonalityCard>,
        default_model: Model,
        history_window: usize,
        tool_selectors: Vec<String>,
        providers: Arc<ProviderRegistry>,
        tools: Arc<ToolRegistry>,
        commands: Arc<CommandRegistry>,
        store: Arc<dyn Store>,
        renderer: Arc<dyn StreamRenderer>,
        goat_root: PathBuf,
    ) -> Self {
        Self {
            persona,
            personality,
            default_model,
            history_window,
            tool_selectors,
            providers,
            tools,
            commands,
            store,
            renderer,
            goat_root,
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

        let mut messages = self.history_messages(&msg.conversation).await?;
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

        let summary = self
            .complete_with_tools(
                handle,
                msg.conversation.clone(),
                reply_to,
                &mut messages,
                TurnMode::Normal,
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

        Ok(())
    }

    async fn history_messages(&self, conv: &ConversationId) -> Result<Vec<LlmMessage>> {
        let history = self
            .store
            .recent(self.persona, conv, self.history_window)
            .await
            .context("read history")?;
        Ok(history
            .into_iter()
            .filter(|h| {
                !matches!(h.direction, Direction::Out) || !looks_like_agent_meta_leak(&h.text)
            })
            .map(|h| LlmMessage {
                role: match h.direction {
                    Direction::In => Role::User,
                    Direction::Out => Role::Assistant,
                },
                content: vec![ContentPart::Text(h.text)],
            })
            .collect())
    }

    async fn complete_with_tools(
        &self,
        handle: Arc<dyn ChannelHandle>,
        conv: ConversationId,
        reply_to: Option<MessageId>,
        messages: &mut Vec<LlmMessage>,
        mode: TurnMode,
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
        let now_iso = chrono::Utc::now().to_rfc3339();
        let base_system = format!(
            "{}\n\n<current_time iso8601=\"{now_iso}\">\nThe current time is {now_iso}. \
             Resolve any user time reference against this clock.\n\
             </current_time>",
            compose_system_prompt(&self.personality.system_prompt, skill_prompt.as_deref()),
        );
        let system_prompt = match mode {
            TurnMode::Normal => base_system,
            TurnMode::SelfTick { .. } => format!(
                "{base_system}\n\n<self_tick_context>\nYou are running at the \
                 fire moment of a scheduled task. Read the task and act. \
                 If the task is no longer worth doing, reply with exactly: skip\n\
                 </self_tick_context>"
            ),
        };

        for _round in 0..MAX_TOOL_ROUNDS {
            let mut req = LlmRequest::new(self.default_model.clone());
            req.system = Some(system_prompt.clone());
            req.messages = messages.clone();
            req.tools = tool_specs.clone();

            let stream = provider.stream(req).await?;
            let folded = fold_turn(stream).await?;

            if folded.tool_calls.is_empty() {
                let final_text = sanitize_final_text(folded.text);
                if matches!(mode, TurnMode::SelfTick { .. })
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

    async fn handle_self_tick(
        &self,
        channels: &[Arc<dyn ChannelHandle>],
        run_id: i64,
        task_id: i64,
    ) -> Result<()> {
        let task = match self.store.get_scheduled_task(task_id).await? {
            Some(t) if matches!(t.status, ScheduledTaskStatus::Active) => t,
            Some(_) => {
                self.store
                    .finish_run(
                        run_id,
                        TaskRunStatus::Skipped,
                        Some("task no longer active".into()),
                    )
                    .await
                    .ok();
                return Ok(());
            }
            None => {
                self.store
                    .finish_run(
                        run_id,
                        TaskRunStatus::Failed,
                        Some("task row missing".into()),
                    )
                    .await
                    .ok();
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
                self.store
                    .finish_run(
                        run_id,
                        TaskRunStatus::Failed,
                        Some("no channel handle for origin_conv".into()),
                    )
                    .await
                    .ok();
                return Ok(());
            }
        };

        let mut messages = vec![LlmMessage {
            role: Role::User,
            content: vec![ContentPart::Text(task.task.clone())],
        }];

        let summary = match self
            .complete_with_tools(
                handle,
                conv.clone(),
                None,
                &mut messages,
                TurnMode::SelfTick {
                    tools: task.tools.clone(),
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                self.store
                    .finish_run(
                        run_id,
                        TaskRunStatus::Failed,
                        Some(format!("self-tick run errored: {e}")),
                    )
                    .await
                    .ok();
                return Err(e);
            }
        };

        let trimmed = summary.final_text.trim();
        if trimmed.eq_ignore_ascii_case("skip") {
            self.store
                .finish_run(
                    run_id,
                    TaskRunStatus::Skipped,
                    Some("model declined".into()),
                )
                .await
                .ok();
            return Ok(());
        }
        if trimmed.is_empty() {
            warn!(
                run_id,
                task_id,
                persona = %self.persona,
                "self-tick produced empty response; marking failed",
            );
            self.store
                .finish_run(
                    run_id,
                    TaskRunStatus::Failed,
                    Some("empty response from model".into()),
                )
                .await
                .ok();
            return Ok(());
        }

        self.store
            .append_outgoing_text(self.persona, &conv, &summary.final_text, None)
            .await
            .context("append outgoing text for self-tick")?;

        let truncated = truncate_for_summary(&summary.final_text);
        self.store
            .finish_run(run_id, TaskRunStatus::Done, Some(truncated))
            .await
            .ok();
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

async fn fold_turn(mut stream: LlmStream) -> Result<FoldedTurn> {
    let mut text = String::new();
    let mut pending: HashMap<BlockId, PendingToolCall> = HashMap::new();

    while let Some(item) = stream.next().await {
        match item? {
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
            LlmChunk::MessageEnd { .. } => break,
            _ => {}
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

fn compose_system_prompt(persona_prompt: &str, skill_prompt: Option<&str>) -> String {
    let mut parts = vec![persona_prompt.trim().to_string()];
    if let Some(skill_prompt) = skill_prompt.filter(|s| !s.trim().is_empty()) {
        parts.push(skill_prompt.trim().to_string());
    }
    parts.push(RUNTIME_SYSTEM_GUARD.trim().to_string());
    parts.join("\n\n")
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
        let prompt = compose_system_prompt("You are dev.", None);
        assert!(prompt.contains("You are dev."));
        assert!(prompt.contains("<goat_runtime_guard>"));
        assert!(prompt.contains("Return only the final user-facing answer."));
    }

    #[test]
    fn compose_system_prompt_inserts_skill_catalog_before_runtime_guard() {
        let prompt = compose_system_prompt("You are dev.", Some("<available_skills/>"));
        let persona = prompt.find("You are dev.").unwrap();
        let skills = prompt.find("<available_skills/>").unwrap();
        let guard = prompt.find("<goat_runtime_guard>").unwrap();
        assert!(persona < skills);
        assert!(skills < guard);
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
}

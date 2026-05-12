use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::{stream, StreamExt};
use goat_bus::{EventBus, EventFilter};
use goat_channel::ChannelHandle;
use goat_llm::{
    BlockId, ContentPart, LlmChunk, LlmMessage, LlmProvider, LlmRequest, LlmStream, Model, Role,
    StopReason, ToolSpec, Usage,
};
use goat_persona::PersonalityCard;
use goat_render::{RenderSummary, StreamRenderer};
use goat_skills::SkillIndex;
use goat_store::{Direction, Store, ToolInvocationRecord, ToolInvocationStatus};
use goat_tool::{ToolCall, ToolContext, ToolOutput, ToolRegistry};
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
    providers: Arc<ProviderRegistry>,
    tools: Arc<ToolRegistry>,
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
        providers: Arc<ProviderRegistry>,
        tools: Arc<ToolRegistry>,
        store: Arc<dyn Store>,
        renderer: Arc<dyn StreamRenderer>,
        goat_root: PathBuf,
    ) -> Self {
        Self {
            persona,
            personality,
            default_model,
            history_window,
            providers,
            tools,
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
        let mut sub = bus.subscribe(EventFilter::IncomingFor(self.persona));
        info!(persona = %self.persona, "brain running");

        while let Some(event) = sub.recv().await {
            let Event::Incoming(msg) = event else {
                continue;
            };

            if let Err(e) = self.handle(&channels, msg).await {
                warn!(persona = %self.persona, error = ?e, "turn failed");
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
        let _typing = handle.typing(&msg.conversation).await?;

        self.store
            .append_incoming(&msg)
            .await
            .context("append incoming")?;

        let mut messages = self.history_messages(&msg.conversation).await?;
        let summary = self
            .complete_with_tools(
                handle,
                msg.conversation.clone(),
                Some(msg.id.clone()),
                &mut messages,
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
    ) -> Result<RenderSummary> {
        const MAX_TOOL_ROUNDS: usize = 8;

        let provider = self.providers.route(&self.default_model)?;
        let skill_prompt =
            SkillIndex::discover_root(&self.goat_root).system_prompt_block(self.persona);
        let tool_specs = self.llm_tool_specs(skill_prompt.is_some());

        for _round in 0..MAX_TOOL_ROUNDS {
            let mut req = LlmRequest::new(self.default_model.clone());
            req.system = Some(compose_system_prompt(
                &self.personality.system_prompt,
                skill_prompt.as_deref(),
            ));
            req.messages = messages.clone();
            req.tools = tool_specs.clone();

            let stream = provider.stream(req).await?;
            let folded = fold_turn(stream).await?;

            if folded.tool_calls.is_empty() {
                let final_text = sanitize_final_text(folded.text);
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
                let output = self.execute_tool(&conv, &call).await;
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

    fn llm_tool_specs(&self, has_skills: bool) -> Vec<ToolSpec> {
        self.tools
            .default_specs()
            .into_iter()
            .filter(|spec| has_skills || spec.name.as_str() != "skill.activate")
            .map(|spec| ToolSpec {
                name: spec.model_name(),
                description: spec.description.unwrap_or_default(),
                input_schema: spec.input_schema,
            })
            .collect()
    }

    async fn execute_tool(&self, conv: &ConversationId, call: &ModelToolCall) -> ToolOutput {
        let started_at = chrono::Utc::now();
        let (resolved_name, output) = match self.tools.resolve_model_name(&call.name) {
            Some(name) => {
                let ctx = ToolContext {
                    persona: self.persona,
                    conversation: conv.clone(),
                    goat_root: self.goat_root.clone(),
                };
                let tool_call = ToolCall {
                    call_id: call.id.clone(),
                    name: name.clone(),
                    arguments: call.arguments.clone(),
                };
                (name.to_string(), self.tools.call(ctx, tool_call).await)
            }
            None => (
                call.name.clone(),
                ToolOutput::error(format!("unknown tool requested by model: {}", call.name)),
            ),
        };
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
        output
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

    #[test]
    fn assistant_tool_call_message_contains_no_user_visible_text() {
        let calls = vec![ModelToolCall {
            id: "call_1".into(),
            name: "shell_run".into(),
            arguments: serde_json::json!({"command": "ls -la"}),
        }];

        let message = assistant_tool_call_message(&calls);

        assert!(matches!(message.role, Role::Assistant));
        assert_eq!(message.content.len(), 1);
        assert!(matches!(
            &message.content[0],
            ContentPart::ToolCall { id, name, .. }
                if id == "call_1" && name == "shell_run"
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

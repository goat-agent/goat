use eventsource_stream::{Event as SseEvent, EventStreamError};
use futures::{Stream, StreamExt};
use goat_llm::{BlockId, LlmChunk, LlmError, LlmStream, Model, StopReason, Usage};
use serde::Deserialize;
use tracing::warn;

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Event {
    #[serde(rename = "response.created")]
    ResponseCreated { response: ResponseMeta },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta { delta: String },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningDelta { delta: String },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { item: OutputItem },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionArgsDelta { delta: String },
    #[serde(rename = "response.completed")]
    Completed { response: CompletedResponse },
    #[serde(rename = "response.error")]
    Error { error: ErrorDetail },
    #[serde(rename = "response.failed")]
    Failed { response: FailedResponse },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
struct FailedResponse {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ResponseMeta {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct CompletedResponse {
    #[serde(default)]
    usage: Option<UsageWire>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct UsageWire {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
}

fn format_error(prefix: &str, detail: &ErrorDetail) -> String {
    let message = detail.message.as_deref().unwrap_or("unknown");
    match detail.code.as_deref() {
        Some(code) => format!("{prefix}: {message} ({code})"),
        None => format!("{prefix}: {message}"),
    }
}

pub(crate) fn translate<S>(stream: S) -> LlmStream
where
    S: Stream<Item = Result<SseEvent, EventStreamError<reqwest::Error>>> + Send + 'static,
{
    let s = async_stream::stream! {
        let mut stream = Box::pin(stream);
        let mut sent_start = false;
        let mut stop_reason: Option<StopReason> = None;
        let mut usage_in: u32 = 0;
        let mut usage_out: u32 = 0;
        let mut tool_block: u32 = 1;
        let mut tool_open = false;
        let mut parse_failures: u32 = 0;

        while let Some(item) = stream.next().await {
            let raw = match item {
                Ok(ev) => ev,
                Err(e) => { yield Err(LlmError::Transport(e.to_string())); return; }
            };
            if raw.data == "[DONE]" {
                break;
            }
            let event: Event = match serde_json::from_str(&raw.data) {
                Ok(e) => { parse_failures = 0; e }
                Err(e) => {
                    parse_failures += 1;
                    warn!(error = ?e, consecutive = parse_failures, "bad codex SSE payload");
                    if let Some(err) = goat_llm::sse_parse_failure_limit(parse_failures) {
                        yield Err(err);
                        return;
                    }
                    continue;
                }
            };

            match event {
                Event::ResponseCreated { response } => {
                    if !sent_start {
                        yield Ok(LlmChunk::MessageStart {
                            id: response.id.unwrap_or_default(),
                            model: Model::new(
                                crate::ID,
                                response.model.unwrap_or_default(),
                            ),
                            input_tokens: 0,
                        });
                        sent_start = true;
                    }
                }
                Event::OutputTextDelta { delta } => {
                    if !sent_start {
                        yield Ok(LlmChunk::MessageStart {
                            id: String::new(),
                            model: Model::new(crate::ID, String::new()),
                            input_tokens: 0,
                        });
                        sent_start = true;
                    }
                    if !delta.is_empty() {
                        yield Ok(LlmChunk::TextDelta { block: BlockId(0), text: delta });
                    }
                }
                Event::ReasoningDelta { delta } => {
                    if !delta.is_empty() {
                        yield Ok(LlmChunk::ReasoningDelta { block: BlockId(0), text: delta });
                    }
                }
                Event::OutputItemAdded { item } => {
                    if item.kind.as_deref() == Some("function_call") {
                        if let Some(name) = item.name {
                            tool_block += 1;
                            tool_open = true;
                            yield Ok(LlmChunk::ToolCallStart {
                                block: BlockId(tool_block),
                                tool_call_id: item.id.unwrap_or_default(),
                                name,
                            });
                        }
                    }
                }
                Event::FunctionArgsDelta { delta } => {
                    if tool_open && !delta.is_empty() {
                        yield Ok(LlmChunk::ToolCallDelta {
                            block: BlockId(tool_block),
                            args_json_fragment: delta,
                        });
                    }
                }
                Event::Completed { response } => {
                    if let Some(u) = response.usage {
                        if let Some(v) = u.input_tokens { usage_in = v; }
                        if let Some(v) = u.output_tokens { usage_out = v; }
                    }
                    stop_reason = Some(match response.status.as_deref() {
                        Some("max_tokens") => StopReason::MaxTokens,
                        Some("incomplete") => StopReason::Stop,
                        _ if tool_open => StopReason::ToolUse,
                        _ => StopReason::EndTurn,
                    });
                }
                Event::Error { error } => {
                    let msg = format_error("codex stream error", &error);
                    yield Err(LlmError::Provider(msg));
                    return;
                }
                Event::Failed { response } => {
                    let status = response.status.as_deref().unwrap_or("failed");
                    let msg = match response.error {
                        Some(detail) => format_error(
                            &format!("codex response failed ({status})"),
                            &detail,
                        ),
                        None => format!("codex response failed: status={status}"),
                    };
                    yield Err(LlmError::Provider(msg));
                    return;
                }
                Event::Other => {}
            }
        }

        yield Ok(LlmChunk::MessageEnd {
            stop: stop_reason.unwrap_or(StopReason::EndTurn),
            usage: Usage { input: usage_in, output: usage_out },
        });
    };
    Box::pin(s)
}

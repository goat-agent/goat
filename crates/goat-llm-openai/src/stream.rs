use std::collections::HashSet;

use eventsource_stream::{Event as SseEvent, EventStreamError};
use futures::{Stream, StreamExt};
use goat_llm::{BlockId, LlmChunk, LlmError, LlmStream, Model, StopReason, Usage};
use serde::Deserialize;
use tracing::warn;

use crate::error::parse_stop;

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<UsageWire>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Deserialize)]
struct ToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionCall>,
}

#[derive(Deserialize)]
struct FunctionCall {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct UsageWire {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

pub(crate) fn translate<S>(stream: S) -> LlmStream
where
    S: Stream<Item = Result<SseEvent, EventStreamError<reqwest::Error>>> + Send + 'static,
{
    let s = async_stream::stream! {
        let mut stream = Box::pin(stream);
        let mut sent_start = false;
        let mut tool_started: HashSet<u32> = HashSet::new();
        let mut stop_reason: Option<StopReason> = None;
        let mut usage_in: u32 = 0;
        let mut usage_out: u32 = 0;
        let mut parse_failures: u32 = 0;

        while let Some(item) = stream.next().await {
            let raw = match item {
                Ok(ev) => ev,
                Err(e) => { yield Err(LlmError::Transport(e.to_string())); return; }
            };
            if raw.data == "[DONE]" {
                break;
            }
            let chunk: Chunk = match serde_json::from_str(&raw.data) {
                Ok(c) => { parse_failures = 0; c }
                Err(e) => {
                    parse_failures += 1;
                    warn!(error = ?e, consecutive = parse_failures, "bad openai SSE payload");
                    if let Some(err) = goat_llm::sse_parse_failure_limit(parse_failures) {
                        yield Err(err);
                        return;
                    }
                    continue;
                }
            };

            if !sent_start {
                yield Ok(LlmChunk::MessageStart {
                    id: chunk.id.clone().unwrap_or_default(),
                    model: Model::new(crate::ID, chunk.model.clone().unwrap_or_default()),
                    input_tokens: 0,
                });
                sent_start = true;
            }

            if let Some(u) = chunk.usage {
                if let Some(v) = u.prompt_tokens { usage_in = v; }
                if let Some(v) = u.completion_tokens { usage_out = v; }
            }

            for choice in chunk.choices {
                let Delta { content, reasoning, tool_calls } = choice.delta;
                if let Some(text) = content.filter(|s| !s.is_empty()) {
                    yield Ok(LlmChunk::TextDelta { block: BlockId(0), text });
                }
                if let Some(text) = reasoning.filter(|s| !s.is_empty()) {
                    yield Ok(LlmChunk::ReasoningDelta { block: BlockId(0), text });
                }
                if let Some(calls) = tool_calls {
                    for tc in calls {
                        let idx = tc.index;
                        let block = BlockId(idx + 1);
                        let name = tc.function.as_ref().and_then(|f| f.name.clone()).filter(|s| !s.is_empty());
                        let args = tc.function.and_then(|f| f.arguments).filter(|s| !s.is_empty());
                        if let Some(name) = name {
                            if tool_started.insert(idx) {
                                yield Ok(LlmChunk::ToolCallStart {
                                    block,
                                    tool_call_id: tc.id.unwrap_or_default(),
                                    name,
                                });
                            }
                        }
                        if let Some(args) = args {
                            yield Ok(LlmChunk::ToolCallDelta {
                                block,
                                args_json_fragment: args,
                            });
                        }
                    }
                }
                if let Some(reason) = choice.finish_reason {
                    stop_reason = Some(parse_stop(&reason));
                }
            }
        }

        yield Ok(LlmChunk::MessageEnd {
            stop: stop_reason.unwrap_or(StopReason::EndTurn),
            usage: Usage { input: usage_in, output: usage_out },
        });
    };
    Box::pin(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chunk_with_content() {
        let d = r#"{"id":"c1","model":"gpt-x","choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        assert_eq!(chunk.id.as_deref(), Some("c1"));
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn parses_chunk_with_finish_and_usage() {
        let d = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":7}}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
        let u = chunk.usage.unwrap();
        assert_eq!(u.prompt_tokens, Some(4));
        assert_eq!(u.completion_tokens, Some(7));
    }

    #[test]
    fn parses_chunk_with_tool_call() {
        let d = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"sum","arguments":"{\"a\":1"}}]}}]}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_1"));
        let f = tc.function.as_ref().unwrap();
        assert_eq!(f.name.as_deref(), Some("sum"));
        assert_eq!(f.arguments.as_deref(), Some("{\"a\":1"));
    }

    #[test]
    fn parses_chunk_with_reasoning() {
        let d = r#"{"choices":[{"delta":{"reasoning":"thinking"}}]}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        assert_eq!(
            chunk.choices[0].delta.reasoning.as_deref(),
            Some("thinking")
        );
    }
}

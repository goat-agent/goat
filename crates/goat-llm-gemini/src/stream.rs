use eventsource_stream::{Event as SseEvent, EventStreamError};
use futures::{Stream, StreamExt};
use goat_llm::{BlockId, LlmChunk, LlmError, LlmStream, Model, StopReason, Usage};
use serde::Deserialize;
use tracing::warn;

use crate::error::parse_stop;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chunk {
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: Option<CandidateContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct CandidateContent {
    #[serde(default)]
    parts: Vec<PartWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartWire {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thought: Option<bool>,
    #[serde(default)]
    function_call: Option<FunctionCall>,
}

#[derive(Deserialize)]
struct FunctionCall {
    name: String,
    #[serde(default)]
    args: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: Option<u32>,
    #[serde(default)]
    candidates_token_count: Option<u32>,
}

pub(crate) fn translate<S>(stream: S, model_id: String) -> LlmStream
where
    S: Stream<Item = Result<SseEvent, EventStreamError<reqwest::Error>>> + Send + 'static,
{
    let s = async_stream::stream! {
        let mut stream = Box::pin(stream);
        let mut sent_start = false;
        let mut input_tokens: u32 = 0;
        let mut output_tokens: u32 = 0;
        let mut stop_reason: Option<StopReason> = None;
        let mut next_tool_block: u32 = 1;
        let mut parse_failures: u32 = 0;

        while let Some(item) = stream.next().await {
            let raw = match item {
                Ok(ev) => ev,
                Err(e) => { yield Err(LlmError::Transport(e.to_string())); return; }
            };
            let chunk: Chunk = match serde_json::from_str(&raw.data) {
                Ok(c) => { parse_failures = 0; c }
                Err(e) => {
                    parse_failures += 1;
                    warn!(error = ?e, consecutive = parse_failures, "bad gemini SSE payload");
                    if let Some(err) = goat_llm::sse_parse_failure_limit(parse_failures) {
                        yield Err(err);
                        return;
                    }
                    continue;
                }
            };

            if !sent_start {
                let initial_input = chunk
                    .usage_metadata
                    .as_ref()
                    .and_then(|u| u.prompt_token_count)
                    .unwrap_or(0);
                yield Ok(LlmChunk::MessageStart {
                    id: chunk.response_id.clone().unwrap_or_default(),
                    model: Model::new(crate::ID, model_id.clone()),
                    input_tokens: initial_input,
                });
                sent_start = true;
            }

            if let Some(u) = chunk.usage_metadata {
                if let Some(v) = u.prompt_token_count { input_tokens = v; }
                if let Some(v) = u.candidates_token_count { output_tokens = v; }
            }

            for cand in chunk.candidates {
                if let Some(content) = cand.content {
                    for part in content.parts {
                        if let Some(call) = part.function_call {
                            let block = BlockId(next_tool_block);
                            next_tool_block += 1;
                            yield Ok(LlmChunk::ToolCallStart {
                                block,
                                tool_call_id: format!("call_{}", block.0),
                                name: call.name,
                            });
                            yield Ok(LlmChunk::ToolCallDelta {
                                block,
                                args_json_fragment: serde_json::to_string(&call.args)
                                    .unwrap_or_else(|_| "{}".into()),
                            });
                            yield Ok(LlmChunk::BlockEnd { block });
                            continue;
                        }
                        let Some(text) = part.text else { continue };
                        if text.is_empty() { continue; }
                        if part.thought.unwrap_or(false) {
                            yield Ok(LlmChunk::ReasoningDelta {
                                block: BlockId(0),
                                text,
                            });
                        } else {
                            yield Ok(LlmChunk::TextDelta {
                                block: BlockId(0),
                                text,
                            });
                        }
                    }
                }
                if let Some(reason) = cand.finish_reason {
                    stop_reason = Some(parse_stop(&reason));
                }
            }
        }

        yield Ok(LlmChunk::MessageEnd {
            stop: stop_reason.unwrap_or(StopReason::EndTurn),
            usage: Usage { input: input_tokens, output: output_tokens },
        });
    };
    Box::pin(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_chunk_with_usage() {
        let d = r#"{"responseId":"r1","candidates":[{"content":{"parts":[{"text":"hi"}]}}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":1}}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        assert_eq!(chunk.response_id.as_deref(), Some("r1"));
        let part = &chunk.candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(part.text.as_deref(), Some("hi"));
        assert!(part.thought.is_none());
        let u = chunk.usage_metadata.unwrap();
        assert_eq!(u.prompt_token_count, Some(4));
        assert_eq!(u.candidates_token_count, Some(1));
    }

    #[test]
    fn parses_thought_flag() {
        let d = r#"{"candidates":[{"content":{"parts":[{"text":"reasoning","thought":true}]}}]}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        let part = &chunk.candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(part.text.as_deref(), Some("reasoning"));
        assert_eq!(part.thought, Some(true));
    }

    #[test]
    fn parses_function_call_atomic() {
        let d = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"sum","args":{"a":1,"b":2}}}]}}]}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        let part = &chunk.candidates[0].content.as_ref().unwrap().parts[0];
        let call = part.function_call.as_ref().unwrap();
        assert_eq!(call.name, "sum");
        assert_eq!(call.args["a"], 1);
        assert_eq!(call.args["b"], 2);
    }

    #[test]
    fn parses_finish_reason() {
        let d = r#"{"candidates":[{"finishReason":"STOP"}]}"#;
        let chunk: Chunk = serde_json::from_str(d).unwrap();
        assert_eq!(chunk.candidates[0].finish_reason.as_deref(), Some("STOP"));
    }
}

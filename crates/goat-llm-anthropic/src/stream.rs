use eventsource_stream::{Event as SseEvent, EventStreamError};
use futures::{Stream, StreamExt};
use goat_llm::{BlockId, LlmChunk, LlmError, LlmStream, Model, StopReason, Usage};
use serde::Deserialize;
use tracing::warn;

use crate::error::parse_stop;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    MessageStart {
        message: MessageMeta,
    },
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: BlockDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaInner,
        #[serde(default)]
        usage: UsageWire,
    },
    MessageStop,
    Error {
        error: ErrorBody,
    },
    Ping,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct MessageMeta {
    id: String,
    model: String,
    #[serde(default)]
    usage: InputUsage,
}

#[derive(Default, Deserialize)]
struct InputUsage {
    #[serde(default)]
    input_tokens: u32,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    ToolUse {
        id: String,
        name: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageDeltaInner {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct UsageWire {
    #[serde(default)]
    output_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

pub(crate) fn translate<S>(stream: S) -> LlmStream
where
    S: Stream<Item = Result<SseEvent, EventStreamError<reqwest::Error>>> + Send + 'static,
{
    let s = async_stream::stream! {
        let mut stream = Box::pin(stream);
        let mut pending_stop: Option<StopReason> = None;
        let mut pending_output: u32 = 0;
        let mut input_tokens: u32 = 0;

        while let Some(item) = stream.next().await {
            let raw = match item {
                Ok(ev) => ev,
                Err(e) => { yield Err(LlmError::Transport(e.to_string())); return; }
            };
            let event = match serde_json::from_str::<Event>(&raw.data) {
                Ok(e) => e,
                Err(e) => { warn!(error = ?e, "bad anthropic SSE payload"); continue; }
            };
            match event {
                Event::Ping | Event::Unknown => {}
                Event::MessageStart { message } => {
                    input_tokens = message.usage.input_tokens;
                    yield Ok(LlmChunk::MessageStart {
                        id: message.id,
                        model: Model::new(crate::ID, message.model),
                        input_tokens,
                    });
                }
                Event::ContentBlockStart { index, content_block } => {
                    if let ContentBlock::ToolUse { id, name } = content_block {
                        yield Ok(LlmChunk::ToolCallStart {
                            block: BlockId(index),
                            tool_call_id: id,
                            name,
                        });
                    }
                }
                Event::ContentBlockDelta { index, delta } => match delta {
                    BlockDelta::TextDelta { text } if !text.is_empty() => {
                        yield Ok(LlmChunk::TextDelta { block: BlockId(index), text });
                    }
                    BlockDelta::ThinkingDelta { thinking } if !thinking.is_empty() => {
                        yield Ok(LlmChunk::ReasoningDelta { block: BlockId(index), text: thinking });
                    }
                    BlockDelta::InputJsonDelta { partial_json } => {
                        yield Ok(LlmChunk::ToolCallDelta {
                            block: BlockId(index),
                            args_json_fragment: partial_json,
                        });
                    }
                    _ => {}
                },
                Event::ContentBlockStop { index } => {
                    yield Ok(LlmChunk::BlockEnd { block: BlockId(index) });
                }
                Event::MessageDelta { delta, usage } => {
                    if let Some(reason) = delta.stop_reason {
                        pending_stop = Some(parse_stop(&reason));
                    }
                    if let Some(t) = usage.output_tokens {
                        pending_output = t;
                    }
                }
                Event::MessageStop => {
                    yield Ok(LlmChunk::MessageEnd {
                        stop: pending_stop.unwrap_or(StopReason::EndTurn),
                        usage: Usage { input: input_tokens, output: pending_output },
                    });
                }
                Event::Error { error } => {
                    yield Err(LlmError::Provider(error.message));
                    return;
                }
            }
        }
    };
    Box::pin(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_message_start_event() {
        let d = r#"{"type":"message_start","message":{"id":"m1","model":"claude-x","usage":{"input_tokens":12}}}"#;
        let ev: Event = serde_json::from_str(d).unwrap();
        match ev {
            Event::MessageStart { message } => {
                assert_eq!(message.id, "m1");
                assert_eq!(message.model, "claude-x");
                assert_eq!(message.usage.input_tokens, 12);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_text_delta_event() {
        let d =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let ev: Event = serde_json::from_str(d).unwrap();
        match ev {
            Event::ContentBlockDelta {
                index,
                delta: BlockDelta::TextDelta { text },
            } => {
                assert_eq!(index, 0);
                assert_eq!(text, "hi");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_thinking_delta_event() {
        let d = r#"{"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#;
        let ev: Event = serde_json::from_str(d).unwrap();
        match ev {
            Event::ContentBlockDelta {
                delta: BlockDelta::ThinkingDelta { thinking },
                ..
            } => assert_eq!(thinking, "hmm"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_tool_use_block_start() {
        let d = r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool1","name":"calc"}}"#;
        let ev: Event = serde_json::from_str(d).unwrap();
        match ev {
            Event::ContentBlockStart {
                index,
                content_block: ContentBlock::ToolUse { id, name },
            } => {
                assert_eq!(index, 2);
                assert_eq!(id, "tool1");
                assert_eq!(name, "calc");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_message_delta_with_usage() {
        let d = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#;
        let ev: Event = serde_json::from_str(d).unwrap();
        match ev {
            Event::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("end_turn"));
                assert_eq!(usage.output_tokens, Some(7));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_ping_silently() {
        let ev: Event = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(ev, Event::Ping));
    }

    #[test]
    fn unknown_event_falls_through() {
        let ev: Event = serde_json::from_str(r#"{"type":"future_event"}"#).unwrap();
        assert!(matches!(ev, Event::Unknown));
    }
}

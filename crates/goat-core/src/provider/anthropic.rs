use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{Error, Flow, Reply, Request, Usage};
use crate::message::{Message, Native, Part, Role};

pub const BASE_URL: &str = "https://api.anthropic.com/v1";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const VERSION: &str = "2026-10-21";
const MAX_TOKENS: u32 = 64_000;
const EMPTY_INPUT: &str = "{}";

#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    base: Box<str>,
}

impl Client {
    pub(super) fn new(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').into(),
        }
    }

    pub(super) async fn send(
        &self,
        request: &Request<'_>,
        key: &str,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, Error> {
        let body = serde_json::to_string(&Body::new(request))?;
        let url = format!("{}/messages", self.base);
        let response = super::post(|| {
            self.http
                .post(&url)
                .header("x-api-key", key)
                .header("anthropic-version", VERSION)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
        })
        .await?;

        let mut blocks: Vec<Option<Building>> = Vec::new();
        let mut usage = Usage::default();
        super::read_events(response, |event| {
            let envelope: Envelope = serde_json::from_str(&event.data)?;
            match envelope.r#type.as_str() {
                "message_start" => {
                    let start: MessageStart = serde_json::from_str(&event.data)?;
                    let raw = start.message.usage;
                    usage.input = raw.input_tokens
                        + raw.cache_read_input_tokens
                        + raw.cache_creation_input_tokens;
                    usage.cached = raw.cache_read_input_tokens;
                    usage.output = raw.output_tokens;
                }
                "content_block_start" => {
                    let start: BlockStart = serde_json::from_str(&event.data)?;
                    let block = Building::start(start.content_block)?;
                    if blocks.len() <= start.index {
                        blocks.resize_with(start.index + 1, || None);
                    }
                    blocks[start.index] = block;
                }
                "content_block_delta" => {
                    let delta: BlockDelta = serde_json::from_str(&event.data)?;
                    if let Some(Some(block)) = blocks.get_mut(delta.index) {
                        block.apply(delta.delta, on_text);
                    }
                }
                "message_delta" => {
                    let delta: MessageDelta = serde_json::from_str(&event.data)?;
                    if let Some(output) = delta.usage.and_then(|u| u.output_tokens) {
                        usage.output = output;
                    }
                }
                "message_stop" => return Ok(Flow::Done),
                "error" => {
                    let error: StreamError = serde_json::from_str(&event.data)?;
                    return Err(Error::Stream(format!(
                        "{}: {}",
                        error.error.r#type, error.error.message
                    )));
                }
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;

        let parts = blocks
            .into_iter()
            .flatten()
            .map(Building::finish)
            .collect::<Result<_, _>>()?;
        Ok(Reply { parts, usage })
    }

    pub(super) async fn models(&self, key: &str) -> Result<Vec<Box<str>>, Error> {
        let body = super::get(
            self.http
                .get(format!("{}/models?limit=1000", self.base))
                .header("x-api-key", key)
                .header("anthropic-version", VERSION),
        )
        .await?;
        let listing: Listing = serde_json::from_str(&body)?;
        Ok(listing.data.into_iter().map(|m| m.id.into()).collect())
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: [TextBlock<'a>; 1],
    tools: Vec<Tool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
    messages: Vec<Turn<'a>>,
    cache_control: CacheControl,
    thinking: Thinking,
}

impl<'a> Body<'a> {
    fn new(request: &Request<'a>) -> Self {
        let messages = request
            .all_messages()
            .filter_map(|message| turn(message, message.from_origin(request.origin)))
            .collect();
        Self {
            model: request.model,
            max_tokens: MAX_TOKENS,
            stream: true,
            system: [TextBlock {
                r#type: "text",
                text: request.instructions,
            }],
            tools: request
                .tools
                .iter()
                .map(|spec| Tool {
                    name: spec.name,
                    description: spec.description,
                    input_schema: &spec.schema,
                })
                .collect(),
            tool_choice: (!request.use_tools).then_some(ToolChoice { r#type: "none" }),
            messages,
            cache_control: CacheControl {
                r#type: "ephemeral",
            },
            thinking: Thinking {
                r#type: "adaptive",
                display: "summarized",
            },
        }
    }
}

fn turn(message: &Message, native: bool) -> Option<Turn<'_>> {
    let role = match message.role {
        Role::Assistant => "assistant",
        Role::User | Role::Tool | Role::Summary => "user",
    };
    let content: Vec<Block<'_>> = message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Text { text, .. } | Part::Summary { text, .. } => (!text.trim().is_empty())
                .then_some(Block::Text(TextBlock {
                    r#type: "text",
                    text,
                })),
            Part::Reasoning { native: raw, .. } => native.then_some(Block::Raw(raw.get())),
            Part::ToolCall {
                id, name, input, ..
            } => Some(Block::ToolUse {
                r#type: "tool_use",
                id,
                name,
                input: serde_json::from_str::<&RawValue>(input).unwrap_or_else(|_| empty_input()),
            }),
            Part::ToolResult {
                call_id,
                output,
                is_error,
            } => Some(Block::ToolResult {
                r#type: "tool_result",
                tool_use_id: call_id,
                content: output,
                is_error: *is_error,
            }),
        })
        .collect();
    (!content.is_empty()).then_some(Turn { role, content })
}

fn empty_input() -> &'static RawValue {
    serde_json::from_str(EMPTY_INPUT)
        .unwrap_or_else(|err| unreachable!("{{}} is valid json: {err}"))
}

#[derive(Serialize)]
struct Turn<'a> {
    role: &'static str,
    content: Vec<Block<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Block<'a> {
    Raw(&'a RawValue),
    Text(TextBlock<'a>),
    ToolUse {
        r#type: &'static str,
        id: &'a str,
        name: &'a str,
        input: &'a RawValue,
    },
    ToolResult {
        r#type: &'static str,
        tool_use_id: &'a str,
        content: &'a str,
        is_error: bool,
    },
}

#[derive(Serialize)]
struct TextBlock<'a> {
    r#type: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct Tool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a RawValue,
}

#[derive(Serialize)]
struct ToolChoice {
    r#type: &'static str,
}

#[derive(Serialize)]
struct CacheControl {
    r#type: &'static str,
}

#[derive(Serialize)]
struct Thinking {
    r#type: &'static str,
    display: &'static str,
}

#[derive(Debug)]
enum Building {
    Text(String),
    Thinking {
        thinking: String,
        signature: String,
    },
    Redacted(Box<RawValue>),
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
}

impl Building {
    fn start(raw: Box<RawValue>) -> Result<Option<Self>, Error> {
        Ok(match serde_json::from_str::<StartBlock>(raw.get())? {
            StartBlock::Text { text } => Some(Self::Text(text)),
            StartBlock::Thinking {
                thinking,
                signature,
            } => Some(Self::Thinking {
                thinking,
                signature,
            }),
            StartBlock::RedactedThinking => Some(Self::Redacted(raw)),
            StartBlock::ToolUse { id, name } => Some(Self::ToolUse {
                id,
                name,
                json: String::new(),
            }),
            StartBlock::Other => None,
        })
    }

    fn apply(&mut self, delta: Delta, on_text: &mut dyn FnMut(&str)) {
        match (self, delta) {
            (Self::Text(text), Delta::Text { text: piece }) => {
                on_text(&piece);
                text.push_str(&piece);
            }
            (Self::Thinking { thinking, .. }, Delta::Thinking { thinking: piece }) => {
                thinking.push_str(&piece);
            }
            (Self::Thinking { signature, .. }, Delta::Signature { signature: piece }) => {
                signature.push_str(&piece);
            }
            (Self::ToolUse { json, .. }, Delta::InputJson { partial_json }) => {
                json.push_str(&partial_json);
            }
            _ => {}
        }
    }

    fn finish(self) -> Result<Part, Error> {
        Ok(match self {
            Self::Text(text) => Part::Text {
                text: text.into(),
                native: None,
            },
            Self::Thinking {
                thinking,
                signature,
            } => {
                let native = serde_json::to_string(&ThinkingBlock {
                    r#type: "thinking",
                    thinking: &thinking,
                    signature: &signature,
                })?;
                Part::Reasoning {
                    summary: thinking.into(),
                    native: Native::new(RawValue::from_string(native)?),
                }
            }
            Self::Redacted(raw) => Part::Reasoning {
                summary: "".into(),
                native: Native::new(raw),
            },
            Self::ToolUse { id, name, json } => Part::ToolCall {
                id: id.into(),
                name: name.into(),
                input: if json.trim().is_empty() {
                    EMPTY_INPUT.into()
                } else {
                    json.into()
                },
                native: None,
            },
        })
    }
}

#[derive(Serialize)]
struct ThinkingBlock<'a> {
    r#type: &'static str,
    thinking: &'a str,
    signature: &'a str,
}

#[derive(Deserialize)]
struct Envelope {
    r#type: String,
}

#[derive(Deserialize)]
struct MessageStart {
    message: StartMessage,
}

#[derive(Deserialize)]
struct StartMessage {
    #[serde(default)]
    usage: StartUsage,
}

#[derive(Deserialize, Default)]
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror the Anthropic usage JSON"
)]
struct StartUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct BlockStart {
    index: usize,
    content_block: Box<RawValue>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StartBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    RedactedThinking,
    ToolUse {
        id: String,
        name: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct BlockDelta {
    index: usize,
    delta: Delta,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageDelta {
    usage: Option<DeltaUsage>,
}

#[derive(Deserialize)]
struct DeltaUsage {
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct StreamError {
    error: StreamErrorDetail,
}

#[derive(Deserialize)]
struct StreamErrorDetail {
    r#type: String,
    message: String,
}

#[derive(Deserialize)]
struct Listing {
    data: Vec<ListedModel>,
}

#[derive(Deserialize)]
struct ListedModel {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account;
    use crate::message::Origin;

    #[test]
    fn tool_results_become_user_tool_result_blocks() {
        let message = Message::new(
            Role::Tool,
            None,
            vec![Part::ToolResult {
                call_id: "toolu_1".into(),
                output: "done".into(),
                is_error: false,
            }],
        );
        let json = serde_json::to_string(&turn(&message, false)).unwrap();
        assert_eq!(
            json,
            r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"done","is_error":false}]}"#
        );
    }

    #[test]
    fn thinking_is_replayed_only_for_the_same_origin() {
        let origin = Origin {
            model: "claude-opus-5-5".into(),
            account: account::Id::generate(),
        };
        let message = Message::new(
            Role::Assistant,
            Some(origin),
            vec![
                Part::Reasoning {
                    summary: "t".into(),
                    native: Native::new(
                        RawValue::from_string(
                            r#"{"type":"thinking","thinking":"t","signature":"s"}"#.to_owned(),
                        )
                        .unwrap(),
                    ),
                },
                Part::text("answer"),
            ],
        );
        let same = serde_json::to_string(&turn(&message, true)).unwrap();
        let other = serde_json::to_string(&turn(&message, false)).unwrap();
        assert!(same.contains(r#""signature":"s""#));
        assert!(!other.contains("thinking"));
    }

    #[test]
    fn assistant_without_content_is_skipped() {
        let message = Message::new(
            Role::Assistant,
            None,
            vec![Part::Reasoning {
                summary: "".into(),
                native: Native::new(
                    RawValue::from_string(r#"{"type":"reasoning"}"#.to_owned()).unwrap(),
                ),
            }],
        );
        assert!(turn(&message, false).is_none());
    }

    #[test]
    fn streamed_thinking_keeps_its_signature() {
        let mut block = Building::Thinking {
            thinking: String::new(),
            signature: String::new(),
        };
        block.apply(
            Delta::Thinking {
                thinking: "plan".to_owned(),
            },
            &mut |_| {},
        );
        block.apply(
            Delta::Signature {
                signature: "sig".to_owned(),
            },
            &mut |_| {},
        );
        let Part::Reasoning { native, .. } = block.finish().unwrap() else {
            panic!("expected reasoning");
        };
        assert_eq!(
            native.get().get(),
            r#"{"type":"thinking","thinking":"plan","signature":"sig"}"#
        );
    }
}

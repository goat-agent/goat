use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{Chunk, Error, Flow, Hit, Images, OMITTED_IMAGE, Reply, Request, Stop, Usage};
use crate::image::Image;
use crate::message::{self, Message, Native, Part, Role};

pub const BASE_URL: &str = "https://api.anthropic.com/v1";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const VERSION: &str = "2026-10-21";
const MAX_TOKENS: u32 = 64_000;
const EMPTY_INPUT: &str = "{}";
const SEARCH_TOKENS: u32 = 4096;
const SEARCH_USES: u32 = 2;

#[derive(Debug, Clone)]
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
        on_chunk: &mut dyn FnMut(Chunk<'_>),
    ) -> Result<Reply, Error> {
        let body = super::json(&Body::new(request), request.images)?;
        let url = format!("{}/messages", self.base);
        let response = super::post(
            self.http
                .post(&url)
                .header("x-api-key", key)
                .header("anthropic-version", VERSION)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body),
        )
        .await?;

        let mut blocks: Vec<Option<Building>> = Vec::new();
        let mut usage = Usage::default();
        let mut stop = Stop::Finished;
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
                        block.apply(delta.delta, on_chunk);
                    }
                }
                "content_block_stop" => {
                    let stop: BlockStop = serde_json::from_str(&event.data)?;
                    if let Some(Some(Building::Thinking { .. })) = blocks.get(stop.index) {
                        on_chunk(Chunk::Reasoning("\n"));
                    }
                }
                "message_delta" => {
                    let delta: MessageDelta = serde_json::from_str(&event.data)?;
                    if let Some(output) = delta.usage.and_then(|u| u.output_tokens) {
                        usage.output = output;
                    }
                    if let Some(reason) = delta.delta.and_then(|d| d.stop_reason) {
                        stop = stop_of(&reason);
                    }
                }
                "message_stop" => return Ok(Flow::Done),
                "error" => {
                    let error: StreamError = serde_json::from_str(&event.data)?;
                    return Err(error.into_error());
                }
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;

        if stop == Stop::Truncated
            && let Some(last) = blocks.iter_mut().rev().find(|block| block.is_some())
            && last.as_ref().is_some_and(Building::is_unfinished_call)
        {
            *last = None;
        }
        let parts = blocks
            .into_iter()
            .flatten()
            .map(Building::finish)
            .collect::<Result<_, _>>()?;
        Ok(Reply { parts, usage, stop })
    }

    pub(super) async fn search(
        &self,
        query: &str,
        model: &str,
        key: &str,
    ) -> Result<Vec<Hit>, Error> {
        let body = serde_json::to_vec(&SearchBody {
            model,
            max_tokens: SEARCH_TOKENS,
            stream: true,
            system: [TextBlock {
                r#type: "text",
                text: Cow::Borrowed(super::SEARCH_INSTRUCTIONS),
            }],
            messages: [SearchMessage {
                role: "user",
                content: query,
            }],
            tools: [ServerTool {
                r#type: "web_search_20250305",
                name: "web_search",
                max_uses: SEARCH_USES,
            }],
        })?;
        let url = format!("{}/messages", self.base);
        let response = super::post(
            self.http
                .post(&url)
                .header("x-api-key", key)
                .header("anthropic-version", VERSION)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body),
        )
        .await?;
        let mut found = Vec::new();
        let mut quotes = Vec::new();
        super::read_events(response, |event| {
            let envelope: Envelope = serde_json::from_str(&event.data)?;
            match envelope.r#type.as_str() {
                "content_block_start" => {
                    let start: SearchBlockStart = serde_json::from_str(&event.data)?;
                    if let SearchBlock::WebSearchToolResult { content } = start.content_block {
                        match content {
                            SearchResults::Found(results) => found.extend(
                                results
                                    .into_iter()
                                    .filter_map(|result| Some((result.url?, result.title))),
                            ),
                            SearchResults::Failed { error_code } => {
                                return Err(Error::Stream(format!("web search: {error_code}")));
                            }
                        }
                    }
                }
                "content_block_delta" => {
                    let delta: SearchDelta = serde_json::from_str(&event.data)?;
                    if let SearchDeltaKind::CitationsDelta { citation } = delta.delta {
                        quotes.push(citation);
                    }
                }
                "message_stop" => return Ok(Flow::Done),
                "error" => {
                    let error: StreamError = serde_json::from_str(&event.data)?;
                    return Err(error.into_error());
                }
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;
        Ok(found
            .into_iter()
            .map(|(url, title)| Hit {
                snippet: quotes
                    .iter()
                    .find(|quote| quote.url == url)
                    .map_or_default(|quote| quote.cited_text.clone()),
                title,
                url,
            })
            .collect())
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: [TextBlock<'a>; 1],
    tools: Vec<Tool<'a>>,
    tool_choice: ToolChoice,
    messages: Vec<Turn<'a>>,
    cache_control: CacheControl,
    thinking: Thinking,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig<'a>>,
}

impl<'a> Body<'a> {
    fn new(request: &Request<'a>) -> Self {
        let messages = request
            .all_messages()
            .filter_map(|message| {
                turn(message, message.from_origin(request.origin), request.images)
            })
            .collect();
        Self {
            model: request.model,
            max_tokens: MAX_TOKENS,
            stream: true,
            system: [TextBlock {
                r#type: "text",
                text: Cow::Borrowed(request.instructions),
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
            tool_choice: if request.use_tools {
                ToolChoice::Auto {
                    disable_parallel_tool_use: true,
                }
            } else {
                ToolChoice::None
            },
            messages,
            cache_control: CacheControl {
                r#type: "ephemeral",
            },
            thinking: Thinking {
                r#type: "adaptive",
                display: "summarized",
            },
            output_config: request.effort.map(|effort| OutputConfig { effort }),
        }
    }
}

fn turn<'a>(message: &'a Message, native: bool, images: &'a Images) -> Option<Turn<'a>> {
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
                    text: Cow::Borrowed(text),
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
                images: shown,
                ..
            } => Some(Block::ToolResult {
                r#type: "tool_result",
                tool_use_id: call_id,
                content: if shown.is_empty() {
                    ResultContent::Text(output)
                } else {
                    ResultContent::Blocks(
                        std::iter::once(Block::Text(TextBlock {
                            r#type: "text",
                            text: Cow::Borrowed(output),
                        }))
                        .chain(shown.iter().map(|image| picture(image, images)))
                        .collect(),
                    )
                },
                is_error: *is_error,
            }),
            Part::Image(image) => Some(picture(image, images)),
            Part::Shell {
                command,
                output,
                code,
            } => Some(Block::Text(TextBlock {
                r#type: "text",
                text: Cow::Owned(message::shell(command, output, *code)),
            })),
            Part::File { path, text } => Some(Block::Text(TextBlock {
                r#type: "text",
                text: Cow::Owned(message::file(path, text)),
            })),
        })
        .collect();
    (!content.is_empty()).then_some(Turn { role, content })
}

fn picture<'a>(image: &'a Image, images: &'a Images) -> Block<'a> {
    match images.get(&image.id) {
        Some(data) => Block::Image {
            r#type: "image",
            source: Source {
                r#type: "base64",
                media_type: image.media.as_str(),
                data,
            },
        },
        None => Block::Text(TextBlock {
            r#type: "text",
            text: Cow::Borrowed(OMITTED_IMAGE),
        }),
    }
}

fn stop_of(reason: &str) -> Stop {
    match reason {
        "max_tokens" | "model_context_window_exceeded" => Stop::Truncated,
        "refusal" => Stop::Refused,
        _ => Stop::Finished,
    }
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
        content: ResultContent<'a>,
        is_error: bool,
    },
    Image {
        r#type: &'static str,
        source: Source<'a>,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
enum ResultContent<'a> {
    Text(&'a str),
    Blocks(Vec<Block<'a>>),
}

#[derive(Serialize)]
struct Source<'a> {
    r#type: &'static str,
    media_type: &'static str,
    data: &'a str,
}

#[derive(Serialize)]
struct TextBlock<'a> {
    r#type: &'static str,
    text: Cow<'a, str>,
}

#[derive(Serialize)]
struct Tool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a RawValue,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToolChoice {
    Auto { disable_parallel_tool_use: bool },
    None,
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

#[derive(Serialize)]
struct OutputConfig<'a> {
    effort: &'a str,
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

    fn apply(&mut self, delta: Delta, on_chunk: &mut dyn FnMut(Chunk<'_>)) {
        match (self, delta) {
            (Self::Text(text), Delta::Text { text: piece }) => {
                on_chunk(Chunk::Text(&piece));
                text.push_str(&piece);
            }
            (Self::Thinking { thinking, .. }, Delta::Thinking { thinking: piece }) => {
                on_chunk(Chunk::Reasoning(&piece));
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

    fn is_unfinished_call(&self) -> bool {
        matches!(self, Self::ToolUse { json, .. } if serde_json::from_str::<&RawValue>(json).is_err())
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
struct BlockStop {
    index: usize,
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
    delta: Option<StopDelta>,
}

#[derive(Deserialize)]
struct StopDelta {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct DeltaUsage {
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct StreamError {
    error: StreamErrorDetail,
}

impl StreamError {
    fn into_error(self) -> Error {
        let text = format!("{}: {}", self.error.r#type, self.error.message);
        match self.error.r#type.as_str() {
            "overloaded_error" | "api_error" => Error::Busy(text),
            _ => Error::Stream(text),
        }
    }
}

#[derive(Serialize)]
struct SearchBody<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: [TextBlock<'a>; 1],
    messages: [SearchMessage<'a>; 1],
    tools: [ServerTool; 1],
}

#[derive(Serialize)]
struct SearchMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
struct ServerTool {
    r#type: &'static str,
    name: &'static str,
    max_uses: u32,
}

#[derive(Deserialize)]
struct SearchBlockStart {
    content_block: SearchBlock,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SearchBlock {
    WebSearchToolResult {
        content: SearchResults,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SearchResults {
    Found(Vec<SearchResult>),
    Failed { error_code: String },
}

#[derive(Deserialize)]
struct SearchResult {
    url: Option<String>,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
struct SearchDelta {
    delta: SearchDeltaKind,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SearchDeltaKind {
    CitationsDelta {
        citation: Citation,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Citation {
    #[serde(default)]
    url: String,
    #[serde(default)]
    cited_text: String,
}

#[derive(Deserialize)]
struct StreamErrorDetail {
    r#type: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account;
    use crate::message::Origin;

    #[test]
    fn effort_goes_into_output_config() {
        let config = OutputConfig { effort: "xhigh" };
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"effort":"xhigh"}"#
        );
    }

    #[test]
    fn tool_results_become_user_tool_result_blocks() {
        let message = Message::new(
            Role::Tool,
            None,
            vec![Part::ToolResult {
                call_id: "toolu_1".into(),
                output: "done".into(),
                is_error: false,
                images: Vec::new(),
                trace: Vec::new(),
            }],
        );
        let json = serde_json::to_string(&turn(&message, false, &Images::default())).unwrap();
        assert_eq!(
            json,
            r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"done","is_error":false}]}"#
        );
    }

    #[test]
    fn images_become_base64_image_blocks_in_prompts_and_tool_results() {
        let image = Image {
            id: crate::image::Id::of(b"png"),
            media: crate::image::Media::Png,
        };
        let mut images = Images::default();
        images.insert(image.id.clone(), "iVBO".to_owned());
        let prompt = Message::new(
            Role::User,
            None,
            vec![Part::text("look"), Part::Image(image.clone())],
        );
        let result = Message::new(
            Role::Tool,
            None,
            vec![Part::ToolResult {
                call_id: "toolu_1".into(),
                output: "{}".into(),
                is_error: false,
                images: vec![image],
                trace: Vec::new(),
            }],
        );
        let block =
            r#"{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBO"}}"#;

        assert_eq!(
            serde_json::to_string(&turn(&prompt, false, &images)).unwrap(),
            format!(r#"{{"role":"user","content":[{{"type":"text","text":"look"}},{block}]}}"#)
        );
        assert_eq!(
            serde_json::to_string(&turn(&result, false, &images)).unwrap(),
            format!(
                r#"{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":[{{"type":"text","text":"{{}}"}},{block}],"is_error":false}}]}}"#
            )
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
        let same = serde_json::to_string(&turn(&message, true, &Images::default())).unwrap();
        let other = serde_json::to_string(&turn(&message, false, &Images::default())).unwrap();
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
        assert!(turn(&message, false, &Images::default()).is_none());
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

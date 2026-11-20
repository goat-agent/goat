use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{Chunk, Error, Flow, Images, OMITTED_IMAGE, Reply, Request, Stop, Usage};
use crate::account::codex;
use crate::image::{Image, Media};
use crate::message::{Message, Native, Part, Role};

pub const BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-6.2-astra";
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    base: Box<str>,
    codex_account: Option<Box<str>>,
}

impl Client {
    pub(super) fn new(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').into(),
            codex_account: None,
        }
    }

    pub(super) fn codex(http: reqwest::Client, base: &str, account_id: &str) -> Self {
        Self {
            codex_account: Some(account_id.into()),
            ..Self::new(http, base)
        }
    }

    fn authorize(
        &self,
        builder: reqwest::RequestBuilder,
        bearer: &str,
        session: &str,
    ) -> reqwest::RequestBuilder {
        let builder = builder.bearer_auth(bearer);
        match &self.codex_account {
            Some(account) => builder
                .header("chatgpt-account-id", &**account)
                .header("originator", codex::ORIGINATOR)
                .header("session-id", session),
            None => builder,
        }
    }

    pub(super) async fn send(
        &self,
        request: &Request<'_>,
        bearer: &str,
        on_chunk: &mut dyn FnMut(Chunk<'_>),
    ) -> Result<Reply, Error> {
        let body = super::json(&Body::new(request), request.images)?;
        let url = format!("{}/responses", self.base);
        let response = super::post(
            self.authorize(self.http.post(&url), bearer, request.cache_key)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body),
        )
        .await?;

        let mut parts = Vec::new();
        let mut usage = Usage::default();
        let mut stop = Stop::Finished;
        super::read_events(response, |event| {
            let envelope: Envelope = serde_json::from_str(&event.data)?;
            match envelope.r#type.as_str() {
                "response.output_text.delta" => {
                    let delta: TextDelta = serde_json::from_str(&event.data)?;
                    on_chunk(Chunk::Text(&delta.delta));
                }
                "response.reasoning_summary_text.delta" => {
                    let delta: TextDelta = serde_json::from_str(&event.data)?;
                    on_chunk(Chunk::Reasoning(&delta.delta));
                }
                "response.reasoning_summary_part.done" => on_chunk(Chunk::Reasoning("\n")),
                "response.output_item.done" => {
                    let done: ItemDone = serde_json::from_str(&event.data)?;
                    parts.extend(part(done.item)?);
                }
                "response.completed" => {
                    let completed: Completed = serde_json::from_str(&event.data)?;
                    usage = completed.response.usage.into();
                    return Ok(Flow::Done);
                }
                "response.failed" => {
                    let failed: Failed = serde_json::from_str(&event.data)?;
                    return Err(failed.response.error.into_error());
                }
                "response.incomplete" => {
                    let incomplete: Incomplete = serde_json::from_str(&event.data)?;
                    stop = match incomplete.response.incomplete_details.reason.as_str() {
                        "max_output_tokens" => Stop::Truncated,
                        "content_filter" => Stop::Refused,
                        reason => return Err(Error::Stream(format!("incomplete: {reason}"))),
                    };
                    return Ok(Flow::Done);
                }
                "error" => {
                    let error: ErrorDetail = serde_json::from_str(&event.data)?;
                    return Err(error.into_error());
                }
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;

        Ok(Reply { parts, usage, stop })
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    instructions: &'a str,
    input: Vec<Input<'a>>,
    tools: Vec<Tool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    store: bool,
    stream: bool,
    parallel_tool_calls: bool,
    reasoning: Reasoning<'a>,
    include: [&'static str; 1],
    prompt_cache_key: &'a str,
}

impl<'a> Body<'a> {
    fn new(request: &Request<'a>) -> Self {
        let mut input = Vec::new();
        for message in request.all_messages() {
            push_input(
                &mut input,
                message,
                message.from_origin(request.origin),
                request.images,
            );
        }
        Self {
            model: request.model,
            instructions: request.instructions,
            input,
            tools: request
                .tools
                .iter()
                .map(|spec| Tool {
                    r#type: "function",
                    name: spec.name,
                    description: spec.description,
                    parameters: &spec.schema,
                    strict: true,
                })
                .collect(),
            tool_choice: (!request.use_tools).then_some("none"),
            store: false,
            stream: true,
            parallel_tool_calls: false,
            reasoning: Reasoning {
                summary: "auto",
                effort: request.effort,
            },
            include: ["reasoning.encrypted_content"],
            prompt_cache_key: request.cache_key,
        }
    }
}

fn push_input<'a>(
    input: &mut Vec<Input<'a>>,
    message: &'a Message,
    native: bool,
    images: &'a Images,
) {
    for part in &message.parts {
        let item = match (part, message.role) {
            (
                Part::Text {
                    native: Some(raw), ..
                },
                Role::Assistant,
            ) if native => Input::Raw(raw.get()),
            (Part::Reasoning { native: raw, .. }, _) if native => Input::Raw(raw.get()),
            (
                Part::ToolCall {
                    native: Some(raw), ..
                },
                _,
            ) if native => Input::Raw(raw.get()),
            (Part::Text { text, .. }, Role::Assistant) => Input::Message {
                role: "assistant",
                content: InputContent::Text(text),
            },
            (Part::Text { text, .. }, _) => Input::Message {
                role: "user",
                content: InputContent::Text(text),
            },
            (Part::Summary { text, .. }, _) => Input::Message {
                role: "developer",
                content: InputContent::Text(text),
            },
            (Part::Image(image), _) => Input::Message {
                role: "user",
                content: InputContent::Parts(vec![picture(image, images)]),
            },
            (
                Part::ToolCall {
                    id, name, input, ..
                },
                _,
            ) => Input::Call {
                r#type: "function_call",
                call_id: id,
                name,
                arguments: input,
            },
            (
                Part::ToolResult {
                    call_id,
                    output,
                    images: shown,
                    ..
                },
                _,
            ) => Input::Output {
                r#type: "function_call_output",
                call_id,
                output: if shown.is_empty() {
                    InputContent::Text(output)
                } else {
                    InputContent::Parts(
                        std::iter::once(InputPart::InputText { text: output })
                            .chain(shown.iter().map(|image| picture(image, images)))
                            .collect(),
                    )
                },
            },
            (Part::Reasoning { .. }, _) => continue,
        };
        input.push(item);
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Input<'a> {
    Raw(&'a RawValue),
    Message {
        role: &'static str,
        content: InputContent<'a>,
    },
    Call {
        r#type: &'static str,
        call_id: &'a str,
        name: &'a str,
        arguments: &'a str,
    },
    Output {
        r#type: &'static str,
        call_id: &'a str,
        output: InputContent<'a>,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
enum InputContent<'a> {
    Text(&'a str),
    Parts(Vec<InputPart<'a>>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InputPart<'a> {
    InputText { text: &'a str },
    InputImage { image_url: DataUrl<'a> },
}

struct DataUrl<'a> {
    media: Media,
    base64: &'a str,
}

impl Serialize for DataUrl<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!(
            "data:{};base64,{}",
            self.media.as_str(),
            self.base64
        ))
    }
}

#[derive(Serialize)]
struct Tool<'a> {
    r#type: &'static str,
    name: &'a str,
    description: &'a str,
    parameters: &'a RawValue,
    strict: bool,
}

#[derive(Serialize)]
struct Reasoning<'a> {
    summary: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a str>,
}

#[derive(Deserialize)]
struct Envelope {
    r#type: String,
}

#[derive(Deserialize)]
struct TextDelta {
    delta: String,
}

#[derive(Deserialize)]
struct ItemDone {
    item: Box<RawValue>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Item {
    Message {
        #[serde(default)]
        content: Vec<Content>,
    },
    Reasoning {
        #[serde(default)]
        summary: Vec<SummaryText>,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    OutputText {
        text: String,
    },
    Refusal {
        refusal: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct SummaryText {
    text: String,
}

fn picture<'a>(image: &'a Image, images: &'a Images) -> InputPart<'a> {
    match images.get(&image.id) {
        Some(base64) => InputPart::InputImage {
            image_url: DataUrl {
                media: image.media,
                base64,
            },
        },
        None => InputPart::InputText {
            text: OMITTED_IMAGE,
        },
    }
}

fn part(raw: Box<RawValue>) -> Result<Option<Part>, Error> {
    Ok(match serde_json::from_str::<Item>(raw.get())? {
        Item::Message { content } => {
            let text: String = content
                .into_iter()
                .filter_map(|content| match content {
                    Content::OutputText { text } => Some(text),
                    Content::Refusal { refusal } => Some(refusal),
                    Content::Other => None,
                })
                .collect();
            Some(Part::Text {
                text: text.into(),
                native: Some(Native::new(raw)),
            })
        }
        Item::Reasoning { summary } => Some(Part::Reasoning {
            summary: summary
                .into_iter()
                .map(|s| s.text)
                .collect::<Vec<_>>()
                .join("\n\n")
                .into(),
            native: Native::new(raw),
        }),
        Item::FunctionCall {
            call_id,
            name,
            arguments,
        } => Some(Part::ToolCall {
            id: call_id.into(),
            name: name.into(),
            input: arguments.into(),
            native: Some(Native::new(raw)),
        }),
        Item::Other => None,
    })
}

#[derive(Deserialize)]
struct Completed {
    response: CompletedResponse,
}

#[derive(Deserialize)]
struct CompletedResponse {
    #[serde(default)]
    usage: RawUsage,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    input_tokens_details: InputDetails,
}

#[derive(Deserialize, Default)]
struct InputDetails {
    #[serde(default)]
    cached_tokens: u64,
}

impl From<RawUsage> for Usage {
    fn from(raw: RawUsage) -> Self {
        Self {
            input: raw.input_tokens,
            cached: raw.input_tokens_details.cached_tokens,
            output: raw.output_tokens,
        }
    }
}

#[derive(Deserialize)]
struct Failed {
    response: FailedResponse,
}

#[derive(Deserialize)]
struct FailedResponse {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct Incomplete {
    response: IncompleteResponse,
}

#[derive(Deserialize)]
struct IncompleteResponse {
    incomplete_details: IncompleteDetails,
}

#[derive(Deserialize)]
struct IncompleteDetails {
    reason: String,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: Option<String>,
    message: String,
}

impl ErrorDetail {
    fn describe(&self) -> String {
        match &self.code {
            Some(code) => format!("{code}: {}", self.message),
            None => self.message.clone(),
        }
    }

    fn into_error(self) -> Error {
        let text = self.describe();
        match self.code.as_deref() {
            Some("server_error" | "server_is_overloaded") => Error::Busy(text),
            _ => Error::Stream(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account;
    use crate::message::Origin;

    fn origin(model: &str) -> Origin {
        Origin {
            model: model.into(),
            account: account::Id::generate(),
        }
    }

    #[test]
    fn same_origin_assistant_items_replay_verbatim() {
        let origin = origin("gpt-6.2-astra");
        let raw = RawValue::from_string(
            r#"{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"hi"}]}"#.to_owned(),
        )
        .unwrap();
        let message = Message::new(
            Role::Assistant,
            Some(origin.clone()),
            vec![Part::Text {
                text: "hi".into(),
                native: Some(Native::new(raw)),
            }],
        );
        let mut input = Vec::new();
        let images = Images::default();
        push_input(&mut input, &message, message.from_origin(&origin), &images);
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains(r#""phase":"final_answer""#));
    }

    #[test]
    fn other_origin_drops_reasoning_and_uses_plain_items() {
        let message = Message::new(
            Role::Assistant,
            Some(origin("claude-opus-5-5")),
            vec![
                Part::Reasoning {
                    summary: "thought".into(),
                    native: Native::new(
                        RawValue::from_string(r#"{"type":"thinking"}"#.to_owned()).unwrap(),
                    ),
                },
                Part::ToolCall {
                    id: "toolu_1".into(),
                    name: "read".into(),
                    input: r#"{"path":"a"}"#.into(),
                    native: None,
                },
            ],
        );
        let mut input = Vec::new();
        let images = Images::default();
        push_input(&mut input, &message, false, &images);
        let json = serde_json::to_string(&input).unwrap();
        assert_eq!(
            json,
            r#"[{"type":"function_call","call_id":"toolu_1","name":"read","arguments":"{\"path\":\"a\"}"}]"#
        );
    }

    fn png() -> (Image, Images) {
        let image = Image {
            id: crate::image::Id::of(b"png"),
            media: Media::Png,
        };
        let mut images = Images::default();
        images.insert(image.id.clone(), "iVBO".to_owned());
        (image, images)
    }

    #[test]
    fn an_attached_image_is_a_user_input_image() {
        let (image, images) = png();
        let message = Message::new(Role::User, None, vec![Part::Image(image)]);
        let mut input = Vec::new();
        push_input(&mut input, &message, false, &images);
        assert_eq!(
            serde_json::to_string(&input).unwrap(),
            r#"[{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,iVBO"}]}]"#
        );
    }

    #[test]
    fn a_tool_result_with_an_image_sends_text_and_image_parts() {
        let (image, images) = png();
        let message = Message::new(
            Role::Tool,
            None,
            vec![Part::ToolResult {
                call_id: "call_1".into(),
                output: "{}".into(),
                is_error: false,
                images: vec![image],
            }],
        );
        let mut input = Vec::new();
        push_input(&mut input, &message, false, &images);
        assert_eq!(
            serde_json::to_string(&input).unwrap(),
            r#"[{"type":"function_call_output","call_id":"call_1","output":[{"type":"input_text","text":"{}"},{"type":"input_image","image_url":"data:image/png;base64,iVBO"}]}]"#
        );
    }

    #[test]
    fn an_image_left_out_of_the_request_becomes_a_note() {
        let (image, _) = png();
        let message = Message::new(Role::User, None, vec![Part::Image(image)]);
        let mut input = Vec::new();
        let none = Images::default();
        push_input(&mut input, &message, false, &none);
        assert!(
            serde_json::to_string(&input)
                .unwrap()
                .contains(r#""text":"[image omitted]""#)
        );
    }

    #[test]
    fn function_call_item_becomes_tool_call() {
        let raw = RawValue::from_string(
            r#"{"type":"function_call","id":"fc_1","call_id":"call_1","name":"bash","arguments":"{\"command\":\"ls\"}"}"#.to_owned(),
        )
        .unwrap();
        let Some(Part::ToolCall {
            id,
            name,
            input,
            native,
        }) = part(raw).unwrap()
        else {
            panic!("expected a tool call");
        };
        assert_eq!(
            (&*id, &*name, &*input),
            ("call_1", "bash", r#"{"command":"ls"}"#)
        );
        assert!(native.is_some());
    }

    #[test]
    fn effort_is_sent_only_when_chosen() {
        let reasoning = |effort| {
            serde_json::to_string(&Reasoning {
                summary: "auto",
                effort,
            })
            .unwrap()
        };
        assert_eq!(reasoning(None), r#"{"summary":"auto"}"#);
        assert_eq!(
            reasoning(Some("high")),
            r#"{"summary":"auto","effort":"high"}"#
        );
    }
}

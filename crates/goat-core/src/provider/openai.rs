use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{
    Chunk, Error, Flow, Hit, Images, Limits, OMITTED_IMAGE, Reply, Request, Stop, Usage, Window,
};
use crate::account::codex;
use crate::image::{Image, Media};
use crate::message::{self, Message, Native, Part, Role};

pub const BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-6.2-astra";
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const SEARCH_SESSION: &str = "goat-search";

#[derive(Debug, Clone)]
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
        let limits = limits(response.headers());

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
                "error" => return Err(stream_error(&event.data)),
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;

        Ok(Reply {
            parts,
            usage,
            stop,
            limits,
        })
    }

    pub(super) async fn search(
        &self,
        query: &str,
        model: &str,
        bearer: &str,
    ) -> Result<Vec<Hit>, Error> {
        let body = serde_json::to_vec(&SearchBody {
            model,
            instructions: super::SEARCH_INSTRUCTIONS,
            input: [SearchInput {
                role: "user",
                content: query,
            }],
            tools: [HostedTool {
                r#type: "web_search",
            }],
            store: false,
            stream: true,
            include: ["web_search_call.action.sources"],
        })?;
        let url = format!("{}/responses", self.base);
        let response = super::post(
            self.authorize(self.http.post(&url), bearer, SEARCH_SESSION)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body),
        )
        .await?;
        let mut cited = Vec::new();
        let mut sources = Vec::new();
        super::read_events(response, |event| {
            let envelope: Envelope = serde_json::from_str(&event.data)?;
            match envelope.r#type.as_str() {
                "response.output_item.done" => {
                    let done: SearchItemDone = serde_json::from_str(&event.data)?;
                    match done.item {
                        SearchItem::Message { content } => cited.extend(citations(content)),
                        SearchItem::WebSearchCall { action } => sources.extend(
                            action
                                .into_iter()
                                .flat_map(|action| action.sources)
                                .filter_map(|source| source.url)
                                .map(|url| without_tracking(&url)),
                        ),
                        SearchItem::Other => {}
                    }
                }
                "response.completed" | "response.incomplete" => return Ok(Flow::Done),
                "response.failed" => {
                    let failed: Failed = serde_json::from_str(&event.data)?;
                    return Err(failed.response.error.into_error());
                }
                "error" => return Err(stream_error(&event.data)),
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;
        cited.extend(sources.into_iter().map(|url| Hit {
            title: url.clone(),
            url,
            snippet: String::new(),
        }));
        Ok(cited)
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
                content: InputContent::Text(Cow::Borrowed(text)),
            },
            (Part::Text { text, .. } | Part::Skill { text, .. }, _) => Input::Message {
                role: "user",
                content: InputContent::Text(Cow::Borrowed(text)),
            },
            (Part::Summary { text, .. }, _) => Input::Message {
                role: "developer",
                content: InputContent::Text(Cow::Borrowed(text)),
            },
            (
                Part::Shell {
                    command,
                    output,
                    code,
                },
                _,
            ) => Input::Message {
                role: "user",
                content: InputContent::Text(Cow::Owned(message::shell(command, output, *code))),
            },
            (Part::File { path, text }, _) => Input::Message {
                role: "user",
                content: InputContent::Text(Cow::Owned(message::file(path, text))),
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
                    InputContent::Text(Cow::Borrowed(output))
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
    Text(Cow<'a, str>),
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
struct SearchBody<'a> {
    model: &'a str,
    instructions: &'a str,
    input: [SearchInput<'a>; 1],
    tools: [HostedTool; 1],
    store: bool,
    stream: bool,
    include: [&'static str; 1],
}

#[derive(Serialize)]
struct SearchInput<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
struct HostedTool {
    r#type: &'static str,
}

#[derive(Deserialize)]
struct SearchItemDone {
    item: SearchItem,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SearchItem {
    Message {
        #[serde(default)]
        content: Vec<CitedText>,
    },
    WebSearchCall {
        action: Option<SearchAction>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct CitedText {
    #[serde(default)]
    text: String,
    #[serde(default)]
    annotations: Vec<Annotation>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Annotation {
    UrlCitation {
        url: String,
        #[serde(default)]
        title: String,
        start_index: usize,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct SearchAction {
    #[serde(default)]
    sources: Vec<SearchSource>,
}

#[derive(Deserialize)]
struct SearchSource {
    url: Option<String>,
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

fn citations(content: Vec<CitedText>) -> impl Iterator<Item = Hit> {
    content.into_iter().flat_map(|part| {
        let text: Vec<char> = part.text.chars().collect();
        part.annotations
            .into_iter()
            .filter_map(move |annotation| match annotation {
                Annotation::UrlCitation {
                    url,
                    title,
                    start_index,
                    ..
                } => Some(Hit {
                    title,
                    url: without_tracking(&url),
                    snippet: sentence_before(&text, start_index),
                }),
                Annotation::Other => None,
            })
            .collect::<Vec<_>>()
    })
}

fn sentence_before(text: &[char], at: usize) -> String {
    let mut end = at.min(text.len());
    loop {
        while end > 0 && text[end - 1].is_whitespace() {
            end -= 1;
        }
        let marker = (end > 1 && text[end - 1] == ')')
            .then(|| {
                (0..end - 1)
                    .rev()
                    .find(|&at| text[at] == '(' && text[at + 1] == '[')
            })
            .flatten();
        match marker {
            Some(start) => end = start,
            None => break,
        }
    }
    let line = text[..end]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |at| at + 1);
    let start = text[line..end]
        .windows(2)
        .rposition(|pair| matches!(pair[0], '.' | '!' | '?') && pair[1].is_whitespace())
        .filter(|at| line + at + 2 < end)
        .map_or(line, |at| line + at + 2);
    text[start..end]
        .iter()
        .collect::<String>()
        .trim()
        .to_owned()
}

fn without_tracking(url: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_owned();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !key.starts_with("utm_"))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    parsed.to_string()
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
#[serde(untagged)]
enum ErrorEvent {
    Nested { error: ErrorDetail },
    Flat(ErrorDetail),
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

fn stream_error(data: &str) -> Error {
    match serde_json::from_str(data) {
        Ok(ErrorEvent::Nested { error } | ErrorEvent::Flat(error)) => error.into_error(),
        Err(_) => Error::Stream(data.to_owned()),
    }
}

fn limits(headers: &reqwest::header::HeaderMap) -> Option<Limits> {
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let whole = |name: &str| -> Option<u64> { text(name)?.split('.').next()?.parse().ok() };
    let mut windows: Vec<Window> = ["primary", "secondary"]
        .iter()
        .filter_map(|which| {
            let minutes = whole(&format!("x-codex-{which}-window-minutes")).filter(|m| *m > 0)?;
            let used = whole(&format!("x-codex-{which}-used-percent"))?.min(100);
            Some(Window {
                minutes,
                used_percent: u8::try_from(used).unwrap_or(100),
                resets_at: whole(&format!("x-codex-{which}-reset-at")),
            })
        })
        .collect();
    windows.sort_by_key(|window| window.minutes);
    let plan = text("x-codex-plan-type").map(str::to_owned);
    (plan.is_some() || !windows.is_empty()).then_some(Limits { plan, windows })
}

#[cfg(test)]
mod tests {
    #[test]
    fn plan_limits_come_from_the_codex_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in [
            ("x-codex-plan-type", "pro"),
            ("x-codex-primary-used-percent", "32"),
            ("x-codex-primary-window-minutes", "10080"),
            ("x-codex-primary-reset-at", "1795626187"),
            ("x-codex-secondary-used-percent", "81.5"),
            ("x-codex-secondary-window-minutes", "300"),
            ("x-codex-secondary-reset-at", ""),
        ] {
            headers.insert(name, value.parse().unwrap());
        }

        assert_eq!(
            limits(&headers),
            Some(Limits {
                plan: Some("pro".to_owned()),
                windows: vec![
                    Window {
                        minutes: 300,
                        used_percent: 81,
                        resets_at: None,
                    },
                    Window {
                        minutes: 10080,
                        used_percent: 32,
                        resets_at: Some(1_795_626_187),
                    },
                ],
            })
        );
    }

    #[test]
    fn a_window_of_zero_minutes_is_left_out() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-codex-secondary-window-minutes", "0".parse().unwrap());
        headers.insert("x-codex-secondary-used-percent", "0".parse().unwrap());

        assert_eq!(limits(&headers), None);
    }

    use super::*;

    #[test]
    fn a_citation_snippet_is_the_sentence_it_backs() {
        let text: Vec<char> = "Tokio is a runtime. spawn_blocking runs blocking code on a thread pool. ([docs.rs](https://docs.rs))"
            .chars()
            .collect();
        let at = text.iter().position(|c| *c == '(').unwrap();

        assert_eq!(
            sentence_before(&text, at),
            "spawn_blocking runs blocking code on a thread pool."
        );
    }

    #[test]
    fn a_second_citation_in_a_row_backs_the_same_sentence() {
        let source = "It runs on a pool. ([a](https://a)) ([b](https://b))";
        let text: Vec<char> = source.chars().collect();
        let at = source[..source.find("([b").unwrap()].chars().count();

        assert_eq!(sentence_before(&text, at), "It runs on a pool.");
    }

    #[test]
    fn tracking_parameters_are_dropped_from_urls() {
        assert_eq!(
            without_tracking("https://docs.rs/tokio/?utm_source=openai"),
            "https://docs.rs/tokio/"
        );
        assert_eq!(
            without_tracking("https://example.com/a?q=1&utm_source=openai"),
            "https://example.com/a?q=1"
        );
    }
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
                trace: Vec::new(),
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

    #[test]
    fn stream_errors_show_the_server_message_in_either_shape() {
        let flat = r#"{"type":"error","code":"invalid_prompt","message":"bad input","param":null}"#;
        let nested = r#"{"type":"error","error":{"type":"invalid_request_error","code":"invalid_prompt","message":"bad input"}}"#;
        for data in [flat, nested] {
            assert!(
                matches!(stream_error(data), Error::Stream(text) if text == "invalid_prompt: bad input")
            );
        }
    }

    #[test]
    fn an_unknown_stream_error_shows_the_raw_event() {
        let data = r#"{"type":"error","detail":"nope"}"#;

        assert!(matches!(stream_error(data), Error::Stream(text) if text == data));
    }
}

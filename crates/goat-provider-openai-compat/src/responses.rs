use std::collections::HashMap;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, ContentBlock, Effort, Message, MessageRole, Model,
    Provider, ProviderId, ProviderMetadata, RateLimitSnapshot, Request, SearchResult, StreamChunk,
    StreamError, ToolChoice, ToolDefinition, Usage, ValidateError, Validated, WebSearchOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::common;

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    input: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    parallel_tool_calls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    store: bool,
    stream: bool,
}

fn effort_wire(effort: Effort) -> &'static str {
    match effort {
        Effort::Off => "none",
        other => other.as_str(),
    }
}

#[must_use]
pub fn responses_efforts(model: &str) -> Vec<Effort> {
    let id = model.to_ascii_lowercase();
    if id.starts_with("gpt-5") {
        vec![
            Effort::Off,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ]
    } else if id.starts_with("o3") || id.starts_with("o4") {
        vec![Effort::Low, Effort::Medium, Effort::High]
    } else {
        Vec::new()
    }
}

fn text_item(role: &str, content_kind: &str, text: &str) -> serde_json::Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": content_kind, "text": text }],
    })
}

fn reasoning_input_item(data: &str) -> Option<serde_json::Value> {
    let blob = serde_json::from_str::<serde_json::Value>(data).ok()?;
    let id = blob.get("id")?.as_str()?;
    let encrypted_content = blob.get("encrypted_content")?.as_str()?;
    Some(json!({
        "type": "reasoning",
        "id": id,
        "summary": [],
        "encrypted_content": encrypted_content,
    }))
}

fn append_message_items(
    message: &Message,
    role: &str,
    content_kind: &str,
    input: &mut Vec<serde_json::Value>,
) {
    let mut text = String::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text: chunk } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(chunk);
            }
            ContentBlock::ToolUse {
                id,
                name,
                input: args,
            } => {
                input.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": common::tool_arguments(args),
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                let image = content.iter().find_map(|b| match b {
                    ContentBlock::Image { media_type, data } => Some((media_type, data)),
                    _ => None,
                });
                let output = if let Some((media_type, data)) = image {
                    json!([{
                        "type": "input_image",
                        "image_url": format!("data:{media_type};base64,{data}"),
                    }])
                } else {
                    json!(ContentBlock::tool_result_text(content))
                };
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": output,
                }));
            }
            ContentBlock::RedactedThinking { data } => {
                if let Some(item) = reasoning_input_item(data) {
                    if !text.is_empty() {
                        input.push(text_item(role, content_kind, &text));
                        text.clear();
                    }
                    input.push(item);
                }
            }
            ContentBlock::Image { media_type, data } => {
                if !text.is_empty() {
                    input.push(text_item(role, content_kind, &text));
                    text.clear();
                }
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": [{
                        "type": "input_image",
                        "image_url": format!("data:{media_type};base64,{data}"),
                    }],
                }));
            }
            ContentBlock::Thinking { .. } => {}
        }
    }
    if !text.is_empty() {
        input.push(text_item(role, content_kind, &text));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_body(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    default_instructions: Option<&str>,
    store: bool,
    effort: Option<Effort>,
    choice: ToolChoice,
    system: Option<&str>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> serde_json::Value {
    let mut instructions = String::new();
    if let Some(system) = system
        && !system.is_empty()
    {
        instructions.push_str(system);
    }
    let mut input: Vec<serde_json::Value> = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System => {
                if !instructions.is_empty() {
                    instructions.push('\n');
                }
                instructions.push_str(&message.text_content());
            }
            MessageRole::User => append_message_items(message, "user", "input_text", &mut input),
            MessageRole::Assistant => {
                append_message_items(message, "assistant", "output_text", &mut input);
            }
        }
    }
    let instructions = if instructions.is_empty() {
        default_instructions
    } else {
        Some(instructions.as_str())
    };
    let tools: Vec<serde_json::Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema,
            })
        })
        .collect();
    let has_tools = !tools.is_empty();
    let reasoning =
        effort.map(|effort| json!({ "effort": effort_wire(effort), "summary": "auto" }));
    let include = if reasoning.is_some() {
        vec!["reasoning.encrypted_content"]
    } else {
        Vec::new()
    };
    let tool_choice = match (has_tools, choice) {
        (false, _) => None,
        (true, ToolChoice::None) => Some("none"),
        (true, ToolChoice::Auto) => Some("auto"),
    };
    let request = ResponsesRequest {
        model,
        instructions,
        input,
        tools,
        tool_choice,
        parallel_tool_calls: has_tools,
        reasoning,
        include,
        temperature,
        max_output_tokens: max_tokens,
        store,
        stream: true,
    };
    serde_json::to_value(request).expect("ResponsesRequest is always serializable")
}

pub async fn run_request(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    account_id: Option<&str>,
    body: &serde_json::Value,
    parse_rate_limits: Option<fn(&reqwest::header::HeaderMap) -> Option<RateLimitSnapshot>>,
    extra_headers: &[(&str, &str)],
) -> Result<ChunkStream, StreamError> {
    let mut builder = client
        .post(url)
        .header("Accept", "text/event-stream")
        .json(body);
    if let Some(token) = bearer {
        builder = builder.bearer_auth(token);
    }
    if let Some(account) = account_id {
        builder = builder.header("chatgpt-account-id", account);
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let resp = builder
        .send()
        .await
        .map_err(|err| common::transport(&err))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let detail = resp.text().await.unwrap_or_default();
        let error = common::classify_http(status, &headers, &detail);
        let snapshot = parse_rate_limits.and_then(|parser| parser(&headers));
        return Err(common::with_snapshot_reset(error, snapshot.as_ref()));
    }
    let rate_limits = parse_rate_limits.and_then(|parser| parser(resp.headers()));
    Ok(stream_responses(resp, rate_limits))
}

fn stream_responses(
    response: reqwest::Response,
    rate_limits: Option<RateLimitSnapshot>,
) -> ChunkStream {
    Box::pin(async_stream::try_stream! {
        if let Some(snapshot) = rate_limits {
            yield StreamChunk::RateLimits { snapshot };
        }
        let mut stream = response.bytes_stream().eventsource();
        let mut tool_calls: HashMap<String, (String, String, String)> = HashMap::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => match event.event.as_str() {
                    "response.output_text.delta" => {
                        if let Some(text) = parse_output_delta(&event.data) {
                            yield StreamChunk::TextDelta { text };
                        }
                    }
                    "response.reasoning_summary_text.delta" => {
                        if let Some(text) = parse_output_delta(&event.data) {
                            yield StreamChunk::ThinkingDelta { text };
                        }
                    }
                    "response.output_item.added" => {
                        if let Some(call) = parse_function_call_item(&event.data) {
                            tool_calls.insert(call.item_id, (call.call_id, call.name, String::new()));
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        if let Some(delta) = parse_arguments_delta(&event.data)
                            && let Some(entry) = tool_calls.get_mut(&delta.item_id)
                        {
                            entry.2.push_str(&delta.delta);
                        }
                    }
                    "response.output_item.done" | "response.function_call_arguments.done" => {
                        if let Some(data) = parse_reasoning_item(&event.data) {
                            yield StreamChunk::RedactedThinking { data };
                        }
                        if let Some(item_id) = parse_item_id(&event.data)
                            && let Some((call_id, name, input)) = tool_calls.remove(&item_id)
                        {
                            yield StreamChunk::ToolCall {
                                id: call_id,
                                name,
                                input,
                            };
                        }
                    }
                    "response.completed" => {
                        if let Some(usage) = parse_completed_usage(&event.data) {
                            yield StreamChunk::Usage { usage };
                        }
                        break;
                    }
                    "response.failed" | "error" => {
                        Err(common::classify_stream_error(&event.data))?;
                    }
                    _ => {}
                },
                Err(err) => {
                    Err(goat_provider::StreamError::transport(err.to_string()))?;
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct CompletedEvent {
    response: Option<CompletedResponse>,
}

#[derive(Deserialize)]
struct CompletedResponse {
    usage: Option<ResponseUsage>,
}

#[derive(Deserialize)]
struct ResponseUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    #[serde(default)]
    input_tokens_details: InputTokenDetails,
}

#[derive(Default, Deserialize)]
struct InputTokenDetails {
    #[serde(default)]
    cached_tokens: u32,
}

fn parse_completed_usage(data: &str) -> Option<Usage> {
    let ev = serde_json::from_str::<CompletedEvent>(data).ok()?;
    let u = ev.response?.usage?;
    Some(Usage {
        input_tokens: u.input_tokens.unwrap_or(0),
        output_tokens: u.output_tokens.unwrap_or(0),
        cache_read_tokens: u.input_tokens_details.cached_tokens,
        cache_write_tokens: 0,
    })
}

#[derive(Deserialize)]
struct OutputTextDelta {
    delta: Option<String>,
}

fn parse_output_delta(data: &str) -> Option<String> {
    serde_json::from_str::<OutputTextDelta>(data).ok()?.delta
}

#[derive(Deserialize)]
struct OutputItemAdded {
    item: OutputItem,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
}

struct FunctionCallItem {
    item_id: String,
    call_id: String,
    name: String,
}

fn parse_function_call_item(data: &str) -> Option<FunctionCallItem> {
    let item = serde_json::from_str::<OutputItemAdded>(data).ok()?.item;
    if item.kind != "function_call" {
        return None;
    }
    Some(FunctionCallItem {
        item_id: item.id?,
        call_id: item.call_id?,
        name: item.name?,
    })
}

fn parse_reasoning_item(data: &str) -> Option<String> {
    let item = serde_json::from_str::<ReasoningItemEvent>(data).ok()?.item;
    if item.kind != "reasoning" {
        return None;
    }
    let encrypted_content = item.encrypted_content?;
    serde_json::to_string(&json!({
        "id": item.id,
        "encrypted_content": encrypted_content,
    }))
    .ok()
}

#[derive(Deserialize)]
struct ReasoningItemEvent {
    item: ReasoningItem,
}

#[derive(Deserialize)]
struct ReasoningItem {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    #[serde(default)]
    encrypted_content: Option<String>,
}

#[derive(Deserialize)]
struct ArgumentsDelta {
    item_id: String,
    delta: String,
}

fn parse_arguments_delta(data: &str) -> Option<ArgumentsDelta> {
    serde_json::from_str::<ArgumentsDelta>(data).ok()
}

#[derive(Deserialize)]
struct ItemRef {
    item_id: Option<String>,
    item: Option<OutputItem>,
}

fn parse_item_id(data: &str) -> Option<String> {
    let parsed = serde_json::from_str::<ItemRef>(data).ok()?;
    parsed
        .item_id
        .or_else(|| parsed.item.and_then(|item| item.id))
}

pub struct ResponsesProvider {
    id: ProviderId,
    base_url: String,
    bearer: Option<String>,
    auth: AuthMethod,
    client: reqwest::Client,
    model_filter: Option<fn(&str) -> bool>,
    vision_filter: fn(&str) -> bool,
    catalog: Vec<String>,
    rate_limits_parser: Option<fn(&reqwest::header::HeaderMap) -> Option<RateLimitSnapshot>>,
    context_windows: Vec<(String, u32)>,
    search_model: Option<&'static str>,
    metadata: ProviderMetadata,
    extra_headers: Vec<(String, String)>,
}

impl ResponsesProvider {
    pub fn new(
        id: ProviderId,
        base_url: impl Into<String>,
        bearer: Option<String>,
        auth: AuthMethod,
    ) -> Self {
        Self {
            id,
            base_url: base_url.into(),
            bearer,
            auth,
            client: common::http_client(),
            model_filter: None,
            vision_filter: crate::vision::known_openai_vision_model,
            catalog: Vec::new(),
            rate_limits_parser: None,
            context_windows: Vec::new(),
            search_model: None,
            metadata: ProviderMetadata::default(),
            extra_headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_extra_headers<K, V>(mut self, headers: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.extra_headers = headers
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    #[must_use]
    pub fn with_search_model(mut self, model: &'static str) -> Self {
        self.search_model = Some(model);
        self
    }

    #[must_use]
    pub fn with_model_filter(mut self, filter: fn(&str) -> bool) -> Self {
        self.model_filter = Some(filter);
        self
    }

    #[must_use]
    pub fn with_vision_filter(mut self, filter: fn(&str) -> bool) -> Self {
        self.vision_filter = filter;
        self
    }

    #[must_use]
    pub fn with_catalog(mut self, catalog: &[&str]) -> Self {
        self.catalog = catalog.iter().map(|id| (*id).to_owned()).collect();
        self
    }

    #[must_use]
    pub fn supports_images(&self, model: &str) -> bool {
        (self.vision_filter)(model)
    }

    #[must_use]
    pub fn with_rate_limits_parser(
        mut self,
        parser: fn(&reqwest::header::HeaderMap) -> Option<RateLimitSnapshot>,
    ) -> Self {
        self.rate_limits_parser = Some(parser);
        self
    }

    #[must_use]
    pub fn with_context_windows(mut self, windows: &[(&str, u32)]) -> Self {
        self.context_windows = windows
            .iter()
            .map(|(prefix, window)| ((*prefix).to_owned(), *window))
            .collect();
        self
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

fn build_web_search_body(
    model: &str,
    instructions: Option<&str>,
    query: &str,
) -> serde_json::Value {
    let mut body = json!({
        "model": model,
        "input": [text_item("user", "input_text", query)],
        "tools": [{ "type": "web_search" }],
        "tool_choice": "auto",
    });
    if let Some(instructions) = instructions {
        body["instructions"] = json!(instructions);
    }
    body
}

pub async fn run_web_search(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    account_id: Option<&str>,
    model: &str,
    instructions: Option<&str>,
    query: &str,
) -> Result<WebSearchOutput, StreamError> {
    let body = build_web_search_body(model, instructions, query);
    let mut builder = client.post(url).json(&body);
    if let Some(token) = bearer {
        builder = builder.bearer_auth(token);
    }
    if let Some(account) = account_id {
        builder = builder.header("chatgpt-account-id", account);
    }
    let resp = builder
        .send()
        .await
        .map_err(|err| common::transport(&err))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let detail = resp.text().await.unwrap_or_default();
        return Err(common::classify_http(status, &headers, &detail));
    }
    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| StreamError::other(format!("invalid search response: {err}")))?;
    Ok(WebSearchOutput::from_results(parse_responses_citations(
        &value,
    )))
}

fn parse_responses_citations(value: &serde_json::Value) -> Vec<SearchResult> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Some(output) = value.get("output").and_then(|output| output.as_array()) else {
        return out;
    };
    for item in output {
        let Some(content) = item.get("content").and_then(|content| content.as_array()) else {
            continue;
        };
        for part in content {
            let Some(annotations) = part
                .get("annotations")
                .and_then(|annotations| annotations.as_array())
            else {
                continue;
            };
            for annotation in annotations {
                if annotation.get("type").and_then(|kind| kind.as_str()) != Some("url_citation") {
                    continue;
                }
                let url = annotation
                    .get("url")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                if url.is_empty() || !seen.insert(url.to_owned()) {
                    continue;
                }
                out.push(SearchResult {
                    title: annotation
                        .get("title")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    url: url.to_owned(),
                    snippet: String::new(),
                });
            }
        }
    }
    out
}

#[async_trait]
impl Provider for ResponsesProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn authenticated(&self) -> bool {
        common::authenticated(self.auth, &self.bearer)
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        common::validate_bearer(
            self.client.clone(),
            format!("{}/models", self.base_url),
            self.auth,
            self.bearer.clone(),
        )
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: self.auth,
            images: true,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        self.metadata
    }

    fn supports_images(&self, model: &str) -> bool {
        (self.vision_filter)(model)
    }

    fn supports_web_search(&self) -> bool {
        self.search_model.is_some()
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let client = self.client.clone();
        let url = format!("{}/responses", self.base_url);
        let bearer = self.bearer.clone();
        let model = self.search_model;
        tokio::spawn(async move {
            let Some(model) = model else {
                return Err(StreamError::other("web search is not supported"));
            };
            run_web_search(&client, &url, bearer.as_deref(), None, model, None, &query).await
        })
    }

    fn list_models(&self) -> Vec<String> {
        self.catalog.clone()
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        responses_efforts(model)
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        self.context_windows
            .iter()
            .find(|(prefix, _)| model.starts_with(prefix.as_str()))
            .map(|(_, w)| *w)
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let client = self.client.clone();
        let url = format!("{}/responses", self.base_url);
        let bearer = self.bearer.clone();
        let rate_limits_parser = self.rate_limits_parser;
        let body = build_body(
            &req.model,
            &req.messages,
            &req.tools,
            None,
            false,
            req.effort,
            req.tool_choice,
            req.system.as_deref(),
            req.temperature,
            req.max_tokens,
        );
        let headers: Vec<(&str, &str)> = self
            .extra_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        run_request(
            &client,
            &url,
            bearer.as_deref(),
            None,
            &body,
            rate_limits_parser,
            &headers,
        )
        .await
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        common::discover_models(
            self.client.clone(),
            format!("{}/models", self.base_url),
            self.bearer.clone(),
            self.model_filter,
            self.vision_filter,
            out,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_body, build_web_search_body, parse_arguments_delta, parse_completed_usage,
        parse_function_call_item, parse_item_id, parse_output_delta, parse_reasoning_item,
        parse_responses_citations,
    };
    use goat_provider::{ContentBlock, Message, MessageRole, ToolDefinition};

    #[test]
    fn web_search_body_uses_list_input() {
        let body = build_web_search_body("gpt-search", Some("base"), "find this");
        assert_eq!(body["model"], "gpt-search");
        assert_eq!(body["instructions"], "base");
        assert!(body["input"].is_array());
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][0]["text"], "find this");
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn web_search_body_omits_empty_instructions() {
        let body = build_web_search_body("gpt-search", None, "find this");
        assert!(body.get("instructions").is_none());
    }

    #[test]
    fn extracts_url_citations() {
        let value = serde_json::json!({
            "output": [
                { "type": "web_search_call", "status": "completed" },
                { "type": "message", "content": [
                    { "type": "output_text", "text": "see sources", "annotations": [
                        { "type": "url_citation", "url": "https://a.example", "title": "A" },
                        { "type": "url_citation", "url": "https://a.example", "title": "A dup" },
                        { "type": "url_citation", "url": "https://b.example", "title": "B" }
                    ]}
                ]}
            ]
        });
        let results = parse_responses_citations(&value);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://a.example");
        assert_eq!(results[1].url, "https://b.example");
    }
    use serde_json::json;

    #[test]
    fn parses_output_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hi"}"#;
        assert_eq!(parse_output_delta(data).as_deref(), Some("Hi"));
    }

    #[test]
    fn default_instructions_used_when_no_system_message() {
        let messages = vec![Message::text(MessageRole::User, "hi")];
        let body = build_body(
            "gpt-5.5",
            &messages,
            &[],
            Some("base"),
            false,
            None,
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert_eq!(body["instructions"], "base");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn system_message_overrides_default_instructions() {
        let messages = vec![
            Message::text(MessageRole::System, "be terse"),
            Message::text(MessageRole::User, "hi"),
        ];
        let body = build_body(
            "gpt-5.5",
            &messages,
            &[],
            Some("base"),
            false,
            None,
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert_eq!(body["instructions"], "be terse");
    }

    #[test]
    fn instructions_omitted_when_empty_and_no_default() {
        let messages = vec![Message::text(MessageRole::User, "hi")];
        let body = build_body(
            "gpt-5.5",
            &messages,
            &[],
            None,
            false,
            None,
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert!(body.get("instructions").is_none());
    }

    #[test]
    fn serializes_tool_definitions() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            input_schema: json!({ "type": "object" }),
        }];
        let messages = vec![Message::text(MessageRole::User, "hi")];
        let body = build_body(
            "gpt-5.5",
            &messages,
            &tools,
            None,
            false,
            None,
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
    }

    #[test]
    fn serializes_tool_use_and_result_items() {
        let assistant = Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({ "path": "a.txt" }),
            }],
        };
        let result = Message {
            role: MessageRole::User,
            content: vec![ContentBlock::text_result("call_1", "file body", false)],
        };
        let body = build_body(
            "gpt-5.5",
            &[assistant, result],
            &[],
            None,
            false,
            None,
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert_eq!(body["input"][0]["type"], "function_call");
        assert_eq!(body["input"][0]["call_id"], "call_1");
        assert_eq!(body["input"][0]["name"], "read_file");
        assert_eq!(body["input"][0]["arguments"], r#"{"path":"a.txt"}"#);
        assert_eq!(body["input"][1]["type"], "function_call_output");
        assert_eq!(body["input"][1]["call_id"], "call_1");
        assert_eq!(body["input"][1]["output"], "file body");
    }

    #[test]
    fn reasoning_included_only_when_effort_present() {
        let messages = vec![Message::text(MessageRole::User, "hi")];
        let plain = build_body(
            "gpt-5.5",
            &messages,
            &[],
            None,
            false,
            None,
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert!(plain.get("reasoning").is_none());
        let high = build_body(
            "gpt-5.5",
            &messages,
            &[],
            None,
            false,
            Some(goat_provider::Effort::High),
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert_eq!(high["reasoning"]["effort"], "high");
        assert_eq!(high["reasoning"]["summary"], "auto");
        let off = build_body(
            "gpt-5.5",
            &messages,
            &[],
            None,
            false,
            Some(goat_provider::Effort::Off),
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert_eq!(off["reasoning"]["effort"], "none");
        assert!(plain.get("include").is_none());
        assert_eq!(high["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn reasoning_item_round_trips_through_input() {
        let done =
            r#"{"item":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"ENC"}}"#;
        let data = parse_reasoning_item(done).expect("reasoning item");
        let message = Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::RedactedThinking { data },
                ContentBlock::Text {
                    text: "answer".to_owned(),
                },
            ],
        };
        let body = build_body(
            "gpt-5.5",
            &[message],
            &[],
            None,
            false,
            Some(goat_provider::Effort::High),
            goat_provider::ToolChoice::Auto,
            None,
            None,
            None,
        );
        assert_eq!(body["input"][0]["type"], "reasoning");
        assert_eq!(body["input"][0]["id"], "rs_1");
        assert_eq!(body["input"][0]["encrypted_content"], "ENC");
        assert!(body["input"][0]["summary"].is_array());
        assert_eq!(body["input"][1]["type"], "message");
    }

    #[test]
    fn reasoning_item_without_encrypted_content_is_ignored() {
        let done = r#"{"item":{"type":"reasoning","id":"rs_1","summary":[]}}"#;
        assert!(parse_reasoning_item(done).is_none());
    }

    #[test]
    fn accumulates_function_call_from_stream() {
        let added = r#"{"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file"}}"#;
        let call = parse_function_call_item(added).unwrap();
        assert_eq!(call.item_id, "fc_1");
        assert_eq!(call.call_id, "call_1");
        assert_eq!(call.name, "read_file");

        let first = r#"{"item_id":"fc_1","delta":"{\"path\":"}"#;
        let second = r#"{"item_id":"fc_1","delta":"\"a.txt\"}"}"#;
        let mut buf = String::new();
        for chunk in [first, second] {
            let delta = parse_arguments_delta(chunk).unwrap();
            assert_eq!(delta.item_id, "fc_1");
            buf.push_str(&delta.delta);
        }
        assert_eq!(buf, r#"{"path":"a.txt"}"#);

        let done = r#"{"item_id":"fc_1"}"#;
        assert_eq!(parse_item_id(done).as_deref(), Some("fc_1"));
        let done_item = r#"{"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read_file"}}"#;
        assert_eq!(parse_item_id(done_item).as_deref(), Some("fc_1"));
    }

    #[test]
    fn completed_usage_does_not_map_reasoning_to_cache_write() {
        let data = r#"{"response":{"usage":{
            "input_tokens":100,
            "output_tokens":50,
            "input_tokens_details":{"cached_tokens":20},
            "output_tokens_details":{"reasoning_tokens":30}
        }}}"#;
        let usage = parse_completed_usage(data).expect("usage");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_write_tokens, 0);
    }
}

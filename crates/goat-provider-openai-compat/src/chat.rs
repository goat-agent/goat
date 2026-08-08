use std::collections::HashMap;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, ContentBlock, Effort, Message, MessageRole, Model,
    ModelListSource, Provider, ProviderId, ProviderMetadata, Request, StreamChunk, StreamError,
    Usage, ValidateError, Validated,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::common;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatValidation {
    ModelsEndpoint,
    CatalogOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatDiscovery {
    ModelsEndpoint,
    CatalogOnly,
}

#[derive(Clone)]
struct ChatOptions {
    tools: bool,
    images: bool,
    stream_options: bool,
    reasoning_effort: bool,
    model_filter: Option<fn(&str) -> bool>,
    vision_filter: fn(&str) -> bool,
    effort_options: fn(&str) -> Vec<Effort>,
    effort_wire: fn(Effort) -> Option<&'static str>,
    catalog: Vec<String>,
    context_windows: Vec<(String, u32)>,
    validation: ChatValidation,
    discovery: ChatDiscovery,
    model_list_source: Option<ModelListSource>,
    metadata: ProviderMetadata,
    extra_headers: Vec<(String, String)>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            tools: true,
            images: true,
            stream_options: true,
            reasoning_effort: true,
            model_filter: None,
            vision_filter: crate::vision::known_openai_compatible_vision_model,
            effort_options: default_efforts,
            effort_wire: chat_effort_wire,
            catalog: Vec::new(),
            context_windows: Vec::new(),
            validation: ChatValidation::ModelsEndpoint,
            discovery: ChatDiscovery::ModelsEndpoint,
            model_list_source: None,
            metadata: ProviderMetadata::default(),
            extra_headers: Vec::new(),
        }
    }
}

pub struct OpenAiCompatProvider {
    id: ProviderId,
    base_url: String,
    bearer: Option<String>,
    auth: AuthMethod,
    client: reqwest::Client,
    options: ChatOptions,
}

impl OpenAiCompatProvider {
    pub fn new(
        id: ProviderId,
        base_url: impl Into<String>,
        bearer: Option<String>,
        auth: AuthMethod,
    ) -> Self {
        Self {
            id,
            base_url: normalize_base_url(&base_url.into()),
            bearer,
            auth,
            client: common::http_client(),
            options: ChatOptions::default(),
        }
    }

    pub fn local(provider_id: &'static str, base_url: &'static str) -> Self {
        Self::new(
            ProviderId::from(provider_id),
            base_url,
            None,
            AuthMethod::None,
        )
        .with_metadata(ProviderMetadata {
            env_var: None,
            validation: "local",
            endpoint: Some(base_url),
            oauth: None,
            login_endpoint: None,
            setup: &[],
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    #[must_use]
    pub fn with_tools(mut self, enabled: bool) -> Self {
        self.options.tools = enabled;
        self
    }

    #[must_use]
    pub fn with_images(mut self, enabled: bool) -> Self {
        self.options.images = enabled;
        self
    }

    #[must_use]
    pub fn with_stream_options(mut self, enabled: bool) -> Self {
        self.options.stream_options = enabled;
        self
    }

    #[must_use]
    pub fn with_reasoning_effort(mut self, enabled: bool) -> Self {
        self.options.reasoning_effort = enabled;
        self
    }

    #[must_use]
    pub fn with_model_filter(mut self, filter: fn(&str) -> bool) -> Self {
        self.options.model_filter = Some(filter);
        self
    }

    #[must_use]
    pub fn with_vision_filter(mut self, filter: fn(&str) -> bool) -> Self {
        self.options.vision_filter = filter;
        self
    }

    #[must_use]
    pub fn with_efforts(mut self, efforts: fn(&str) -> Vec<Effort>) -> Self {
        self.options.effort_options = efforts;
        self
    }

    #[must_use]
    pub fn with_effort_wire(mut self, effort_wire: fn(Effort) -> Option<&'static str>) -> Self {
        self.options.effort_wire = effort_wire;
        self
    }

    #[must_use]
    pub fn with_catalog(mut self, catalog: &[&str]) -> Self {
        self.options.catalog = catalog.iter().map(|id| (*id).to_owned()).collect();
        self
    }

    #[must_use]
    pub fn with_context_windows(mut self, windows: &[(&str, u32)]) -> Self {
        self.options.context_windows = windows
            .iter()
            .map(|(prefix, window)| ((*prefix).to_owned(), *window))
            .collect();
        self
    }

    #[must_use]
    pub fn with_validation(mut self, validation: ChatValidation) -> Self {
        self.options.validation = validation;
        self
    }

    #[must_use]
    pub fn with_discovery(mut self, discovery: ChatDiscovery) -> Self {
        self.options.discovery = discovery;
        self
    }

    #[must_use]
    pub fn with_model_list_source(mut self, source: ModelListSource) -> Self {
        self.options.model_list_source = Some(source);
        self
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.options.metadata = metadata;
        self
    }

    #[must_use]
    pub fn with_extra_headers<K, V>(mut self, headers: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.options.extra_headers = headers
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_owned()
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<serde_json::Value>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn default_efforts(_model: &str) -> Vec<Effort> {
    vec![Effort::Low, Effort::Medium, Effort::High]
}

fn chat_effort_wire(effort: Effort) -> Option<&'static str> {
    match effort {
        Effort::Off => None,
        Effort::Low => Some("low"),
        Effort::Medium => Some("medium"),
        Effort::High | Effort::Xhigh | Effort::Max => Some("high"),
    }
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    }
}

fn text_and_images_content(message: &Message) -> serde_json::Value {
    let images: Vec<(&String, &String)> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Image { media_type, data } => Some((media_type, data)),
            _ => None,
        })
        .collect();
    let text = message.text_content();
    if images.is_empty() {
        return serde_json::Value::String(text);
    }
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(json!({ "type": "text", "text": text }));
    }
    for (media_type, data) in images {
        content.push(json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{media_type};base64,{data}") },
        }));
    }
    serde_json::Value::Array(content)
}

fn to_chat_messages_with_system(
    system: Option<&str>,
    messages: &[Message],
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if let Some(system) = system
        && !system.is_empty()
    {
        out.push(json!({ "role": "system", "content": system }));
    }
    out.extend(to_chat_messages(messages));
    out
}

fn to_chat_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for message in messages {
        let has_tool_use = message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { .. }));
        let has_tool_result = message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
        if has_tool_use {
            let tool_calls: Vec<serde_json::Value> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => Some(json!({
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": common::tool_arguments(input) },
                    })),
                    _ => None,
                })
                .collect();
            let text = message.text_content();
            out.push(json!({
                "role": "assistant",
                "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text) },
                "tool_calls": tool_calls,
            }));
        } else if has_tool_result {
            for block in &message.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    let output_text = ContentBlock::tool_result_text(content);
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": output_text,
                    }));
                }
            }
        } else {
            out.push(json!({
                "role": role_label(message.role),
                "content": text_and_images_content(message),
            }));
        }
    }
    out
}

fn to_chat_tools(req: &Request) -> Vec<serde_json::Value> {
    req.tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                },
            })
        })
        .collect()
}

fn request_has_images(req: &Request) -> bool {
    req.messages.iter().any(|message| {
        message.content.iter().any(|block| match block {
            ContentBlock::Image { .. } => true,
            ContentBlock::ToolResult { content, .. } => content
                .iter()
                .any(|item| matches!(item, ContentBlock::Image { .. })),
            _ => false,
        })
    })
}

fn build_chat_body(req: &Request, options: &ChatOptions) -> Result<serde_json::Value, StreamError> {
    if request_has_images(req) && (!options.images || !(options.vision_filter)(&req.model)) {
        return Err(StreamError::invalid_request(
            "this provider or model does not support image input; switch to a vision-capable model",
        ));
    }
    let tools = if options.tools {
        to_chat_tools(req)
    } else {
        Vec::new()
    };
    let tool_choice = (options.tools
        && !req.tools.is_empty()
        && matches!(req.tool_choice, goat_provider::ToolChoice::None))
    .then_some("none");
    let reasoning_effort = if options.reasoning_effort {
        req.effort.and_then(options.effort_wire)
    } else {
        None
    };
    let body = ChatRequest {
        model: &req.model,
        messages: to_chat_messages_with_system(req.system.as_deref(), &req.messages),
        stream: true,
        stream_options: options.stream_options.then_some(StreamOptions {
            include_usage: true,
        }),
        tool_choice,
        tools,
        reasoning_effort,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
    };
    serde_json::to_value(body).map_err(|err| StreamError::other(err.to_string()))
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ChatChoice {
    #[serde(default)]
    delta: ChatDelta,
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChatDelta {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallChunk>,
}

#[derive(Deserialize)]
struct ToolCallChunk {
    index: u32,
    id: Option<String>,
    function: Option<ToolCallFunction>,
}

#[derive(Deserialize)]
struct ToolCallFunction {
    name: Option<String>,
    arguments: Option<String>,
}

type ToolAccumulator = HashMap<u32, (String, String, String)>;

fn accumulate_tool_calls(tool_calls: &mut ToolAccumulator, deltas: Vec<ToolCallChunk>) {
    for call in deltas {
        let entry = tool_calls.entry(call.index).or_default();
        if let Some(id) = call.id {
            entry.0 = id;
        }
        if let Some(function) = call.function {
            if let Some(name) = function.name {
                entry.1 = name;
            }
            if let Some(arguments) = function.arguments {
                entry.2.push_str(&arguments);
            }
        }
    }
}

fn drain_tool_calls(tool_calls: &mut ToolAccumulator) -> Vec<StreamChunk> {
    let mut entries: Vec<(u32, (String, String, String))> = tool_calls.drain().collect();
    entries.sort_by_key(|(index, _)| *index);
    entries
        .into_iter()
        .map(|(_, (id, name, input))| StreamChunk::ToolCall { id, name, input })
        .collect()
}

fn data_has_error(data: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .filter(|error| !error.is_null())
                .map(|_| ())
        })
        .is_some()
}

fn stream_chat(response: reqwest::Response) -> ChunkStream {
    Box::pin(async_stream::try_stream! {
        let mut stream = response.bytes_stream().eventsource();
        let mut tool_calls: ToolAccumulator = HashMap::new();
        let mut last_usage: Option<ChatUsage> = None;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    if event.data == "[DONE]" {
                        break;
                    }
                    if event.event == "error" || data_has_error(&event.data) {
                        Err(common::classify_stream_error(&event.data))?;
                    }
                    let Ok(chunk) = serde_json::from_str::<ChatChunk>(&event.data) else {
                        continue;
                    };
                    if chunk.usage.is_some() {
                        last_usage = chunk.usage;
                    }
                    let Some(choice) = chunk.choices.into_iter().next() else {
                        continue;
                    };
                    let reasoning = choice.delta.reasoning_content.or(choice.delta.reasoning);
                    if let Some(text) = reasoning
                        && !text.is_empty()
                    {
                        yield StreamChunk::ThinkingDelta { text };
                    }
                    if let Some(text) = choice.delta.content {
                        yield StreamChunk::TextDelta { text };
                    }
                    accumulate_tool_calls(&mut tool_calls, choice.delta.tool_calls);
                    if choice.finish_reason.is_some() {
                        for call in drain_tool_calls(&mut tool_calls) {
                            yield call;
                        }
                    }
                }
                Err(err) => {
                    Err(goat_provider::StreamError::transport(err.to_string()))?;
                }
            }
        }
        for call in drain_tool_calls(&mut tool_calls) {
            yield call;
        }
        if let Some(u) = last_usage {
            let usage = Usage {
                input_tokens: u.prompt_tokens.unwrap_or(0),
                output_tokens: u.completion_tokens.unwrap_or(0),
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            };
            yield StreamChunk::Usage { usage };
        }
    })
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn authenticated(&self) -> bool {
        common::authenticated(self.auth, &self.bearer)
    }

    fn verifies_credentials(&self) -> bool {
        matches!(self.options.validation, ChatValidation::ModelsEndpoint)
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        match self.options.validation {
            ChatValidation::ModelsEndpoint => common::validate_bearer(
                self.client.clone(),
                format!("{}/models", self.base_url),
                self.auth,
                self.bearer.clone(),
            ),
            ChatValidation::CatalogOnly => tokio::spawn(async move { Ok(Validated::Assumed) }),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: self.options.tools,
            auth: self.auth,
            images: self.options.images,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        self.options.metadata
    }

    fn list_models(&self) -> Vec<String> {
        self.options.catalog.clone()
    }

    fn model_list_source(&self) -> ModelListSource {
        self.options.model_list_source.unwrap_or({
            if self.options.catalog.is_empty() {
                ModelListSource::Discover
            } else {
                ModelListSource::Catalog
            }
        })
    }

    fn supports_images(&self, model: &str) -> bool {
        self.options.images && (self.options.vision_filter)(model)
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        if self.options.reasoning_effort {
            (self.options.effort_options)(model)
        } else {
            Vec::new()
        }
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        self.options
            .context_windows
            .iter()
            .find_map(|(prefix, window)| model.starts_with(prefix.as_str()).then_some(*window))
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let client = self.client.clone();
        let url = format!("{}/chat/completions", self.base_url);
        let bearer = self.bearer.clone();
        let options = self.options.clone();
        let body = build_chat_body(&req, &options)?;
        let mut builder = client.post(&url).json(&body);
        if let Some(token) = &bearer {
            builder = builder.bearer_auth(token);
        }
        for (name, value) in &options.extra_headers {
            builder = builder.header(name, value);
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
        Ok(stream_chat(resp))
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        match self.options.discovery {
            ChatDiscovery::ModelsEndpoint => common::discover_models(
                self.client.clone(),
                format!("{}/models", self.base_url),
                self.bearer.clone(),
                self.options.model_filter,
                self.options.vision_filter,
                out,
            ),
            ChatDiscovery::CatalogOnly => {
                let catalog = self.options.catalog.clone();
                let vision_filter = self.options.vision_filter;
                let images = self.options.images;
                tokio::spawn(async move {
                    for id in catalog {
                        let supports_images = images && vision_filter(&id);
                        if out
                            .send(Model {
                                id,
                                supports_images,
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChatChunk, ChatDiscovery, ChatOptions, ChatValidation, OpenAiCompatProvider,
        ToolAccumulator, accumulate_tool_calls, build_chat_body, data_has_error, drain_tool_calls,
        to_chat_messages,
    };

    #[test]
    fn null_error_field_is_not_a_stream_error() {
        assert!(!data_has_error(
            r#"{"choices":[{"delta":{"content":"hi"}}],"error":null}"#
        ));
        assert!(!data_has_error(r#"{"choices":[]}"#));
        assert!(data_has_error(r#"{"error":{"message":"boom"}}"#));
    }

    use goat_provider::{
        AuthMethod, ContentBlock, Effort, Message, MessageRole, Provider, Request, StreamChunk,
        ToolChoice, ToolDefinition,
    };
    use serde_json::json;

    fn chunk_tool_calls(data: &str) -> Vec<super::ToolCallChunk> {
        let chunk: ChatChunk = serde_json::from_str(data).unwrap();
        chunk.choices.into_iter().next().unwrap().delta.tool_calls
    }

    #[test]
    fn reasoning_delta_fields_are_parsed() {
        let deepseek: ChatChunk =
            serde_json::from_str(r#"{"choices":[{"delta":{"reasoning_content":"hmm"}}]}"#).unwrap();
        assert_eq!(
            deepseek.choices[0].delta.reasoning_content.as_deref(),
            Some("hmm")
        );
        let openrouter: ChatChunk =
            serde_json::from_str(r#"{"choices":[{"delta":{"reasoning":"think"}}]}"#).unwrap();
        assert_eq!(
            openrouter.choices[0].delta.reasoning.as_deref(),
            Some("think")
        );
    }

    fn request() -> Request {
        Request {
            model: "model".to_owned(),
            messages: vec![Message::text(MessageRole::User, "hi")],
            tools: vec![ToolDefinition {
                name: "read_file".to_owned(),
                description: "read".to_owned(),
                input_schema: json!({ "type": "object" }),
            }],
            effort: Some(Effort::High),
            tool_choice: ToolChoice::None,
            temperature: None,
            max_tokens: None,
            system: None,
        }
    }

    #[test]
    fn normalizes_base_url() {
        let provider = OpenAiCompatProvider::new(
            "test".into(),
            "https://api.example.com/v1/",
            None,
            AuthMethod::ApiKey,
        );
        assert_eq!(provider.base_url(), "https://api.example.com/v1");
    }

    #[test]
    fn validation_mode_controls_verification_hint() {
        let provider = OpenAiCompatProvider::new(
            "test".into(),
            "https://api.example.com/v1",
            Some("key".to_owned()),
            AuthMethod::ApiKey,
        )
        .with_validation(ChatValidation::CatalogOnly);
        assert!(!provider.verifies_credentials());
    }

    #[test]
    fn catalog_only_discovery_returns_catalog_models() {
        const CATALOG: &[&str] = &["a", "b"];
        let provider = OpenAiCompatProvider::new(
            "test".into(),
            "https://api.example.com/v1",
            None,
            AuthMethod::ApiKey,
        )
        .with_catalog(CATALOG)
        .with_discovery(ChatDiscovery::CatalogOnly);
        assert_eq!(provider.list_models(), CATALOG);
    }

    #[test]
    fn build_body_can_omit_provider_specific_fields() {
        let options = ChatOptions {
            tools: false,
            stream_options: false,
            reasoning_effort: false,
            ..ChatOptions::default()
        };
        let body = build_chat_body(&request(), &options).unwrap();
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("stream_options").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn image_content_without_vision_support_errors() {
        let mut req = request();
        req.messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_owned(),
                data: "abc".to_owned(),
            }],
        }];
        let options = ChatOptions {
            images: false,
            ..ChatOptions::default()
        };
        let error = build_chat_body(&req, &options).unwrap_err();
        assert!(matches!(
            error,
            goat_provider::StreamError::InvalidRequest { .. }
        ));
    }

    #[test]
    fn effort_wire_is_serialized() {
        let body = build_chat_body(&request(), &ChatOptions::default()).unwrap();
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn error_chunk_is_detected() {
        assert!(data_has_error(
            r#"{"error":{"message":"bad","type":"invalid_request_error"}}"#
        ));
        assert!(!data_has_error(
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#
        ));
        assert!(!data_has_error("not json"));
    }

    #[test]
    fn plain_text_message_uses_text_role() {
        let messages = vec![Message::text(MessageRole::User, "hi")];
        let out = to_chat_messages(&messages);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
        assert!(out[0].get("tool_calls").is_none());
    }

    #[test]
    fn tool_use_becomes_assistant_tool_calls() {
        let messages = vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({ "path": "a.txt" }),
            }],
        }];
        let out = to_chat_messages(&messages);
        assert_eq!(out[0]["role"], "assistant");
        assert!(out[0]["content"].is_null());
        assert_eq!(out[0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(out[0]["tool_calls"][0]["type"], "function");
        assert_eq!(out[0]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(
            out[0]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"a.txt"}"#
        );
    }

    #[test]
    fn tool_result_becomes_tool_role_message() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::text_result("call_1", "file body")],
        }];
        let out = to_chat_messages(&messages);
        assert_eq!(out[0]["role"], "tool");
        assert_eq!(out[0]["tool_call_id"], "call_1");
        assert_eq!(out[0]["content"], "file body");
    }

    #[test]
    fn accumulates_streamed_tool_call() {
        let mut tool_calls: ToolAccumulator = ToolAccumulator::new();
        let first = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#;
        let second = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.txt\"}"}}]}}]}"#;
        accumulate_tool_calls(&mut tool_calls, chunk_tool_calls(first));
        accumulate_tool_calls(&mut tool_calls, chunk_tool_calls(second));
        let events = drain_tool_calls(&mut tool_calls);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamChunk::ToolCall { id, name, input }
                if id == "call_1" && name == "read_file" && input == r#"{"path":"a.txt"}"#
        ));
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn drains_multiple_tool_calls_in_index_order() {
        let mut tool_calls: ToolAccumulator = ToolAccumulator::new();
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"two","arguments":"{}"}},{"index":0,"id":"a","function":{"name":"one","arguments":"{}"}}]}}]}"#;
        accumulate_tool_calls(&mut tool_calls, chunk_tool_calls(data));
        let events = drain_tool_calls(&mut tool_calls);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamChunk::ToolCall { id, name, input }
                if id == "a" && name == "one" && input == "{}"
        ));
        assert!(matches!(
            &events[1],
            StreamChunk::ToolCall { id, name, input }
                if id == "b" && name == "two" && input == "{}"
        ));
    }

    #[tokio::test]
    async fn stream_sends_extra_headers() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        use futures::StreamExt;
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        const HEADERS: &[(&str, &str)] = &[
            ("User-Agent", "xai-grok-cli"),
            ("x-grok-client-version", "0.2.82"),
            ("x-grok-client-identifier", "xai-grok-cli"),
        ];

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let saw_version = Arc::new(AtomicBool::new(false));
        let saw_identifier = Arc::new(AtomicBool::new(false));
        let saw_user_agent = Arc::new(AtomicBool::new(false));
        let saw_version_server = saw_version.clone();
        let saw_identifier_server = saw_identifier.clone();
        let saw_user_agent_server = saw_user_agent.clone();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16_384];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            if request.contains("x-grok-client-version: 0.2.82") {
                saw_version_server.store(true, Ordering::SeqCst);
            }
            if request.contains("x-grok-client-identifier: xai-grok-cli") {
                saw_identifier_server.store(true, Ordering::SeqCst);
            }
            if request
                .to_ascii_lowercase()
                .contains("user-agent: xai-grok-cli")
            {
                saw_user_agent_server.store(true, Ordering::SeqCst);
            }
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let provider = OpenAiCompatProvider::new(
            "test".into(),
            format!("http://{addr}/v1"),
            None,
            AuthMethod::None,
        )
        .with_extra_headers(HEADERS.iter().copied());
        let mut stream = provider
            .stream(Request {
                model: "grok-composer-2.5-fast".to_owned(),
                messages: vec![Message::text(MessageRole::User, "hi")],
                tools: Vec::new(),
                effort: None,
                tool_choice: ToolChoice::None,
                temperature: None,
                max_tokens: None,
                system: None,
            })
            .await
            .unwrap();
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.next().await {
            chunks.push(chunk.unwrap());
        }
        server.await.unwrap();
        assert!(saw_version.load(Ordering::SeqCst));
        assert!(saw_identifier.load(Ordering::SeqCst));
        assert!(saw_user_agent.load(Ordering::SeqCst));
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::TextDelta { .. }));
    }
}

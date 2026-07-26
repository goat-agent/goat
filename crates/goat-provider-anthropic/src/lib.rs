use std::collections::HashMap;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
mod auth;
mod error;
use futures::StreamExt;
use goat_auth::{CredentialKey, CredentialStore, TokenSet};
use goat_provider::{
    AuthMethod, Capabilities, ChunkStream, ContentBlock, Effort, Message, MessageRole, Model,
    Provider, ProviderId, ProviderMetadata, RateLimitSnapshot, RateWindow, Request, SearchResult,
    StreamChunk, StreamError, Usage, ValidateError, Validated, WebSearchOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

pub use auth::AnthropicAuthError;
use auth::{Auth, current_auth, do_login};

pub const PROVIDER_ID: &str = "anthropic";
const BASE_URL: &str = "https://api.anthropic.com/v1";
pub(crate) const ENV_VAR: &str = "ANTHROPIC_API_KEY";
const VERSION: &str = "2023-06-01";
const WEB_SEARCH_MODEL: &str = "claude-haiku-4-5-20251001";
const MAX_TOKENS: u32 = 16384;

pub(crate) const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub(crate) const OAUTH_AUTHORIZE: &str = "https://claude.ai/oauth/authorize";
pub(crate) const OAUTH_TOKEN: &str = "https://platform.claude.com/v1/oauth/token";
pub(crate) const OAUTH_SCOPE: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const OAUTH_BETA: &str = "oauth-2025-04-20,claude-code-20250219";
const OAUTH_USER_AGENT: &str = "claude-cli/2.1.119 (external, cli)";
pub(crate) const OAUTH_TOKEN_UA: &str = "axios/1.13.6";
const CLAUDE_CODE_SYSTEM: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

fn anthropic_context_window(model: &str) -> u32 {
    let id = model.to_ascii_lowercase();
    if id.contains("fable")
        || id.contains("opus-5")
        || id.contains("opus-4-8")
        || id.contains("opus-4-7")
        || id.contains("opus-4-6")
        || id.contains("sonnet-5")
        || id.contains("sonnet-4-6")
    {
        1_000_000
    } else {
        200_000
    }
}

fn anthropic_supports_images(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    id.starts_with("claude-")
        && !id.contains("text")
        && !id.contains("embedding")
        && !id.contains("search")
}

const CATALOG: &[&str] = &[
    "claude-fable-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-opus-4-5-20251101",
    "claude-sonnet-4-5-20250929",
    "claude-opus-4-1-20250805",
];

pub struct AnthropicProvider {
    base_url: String,
    store: CredentialStore,
    key: CredentialKey,
    client: reqwest::Client,
    inject_identity: bool,
}

impl AnthropicProvider {
    pub fn new(store: CredentialStore, key: CredentialKey) -> Self {
        Self::with_identity(store, key, true)
    }

    pub fn with_identity(
        store: CredentialStore,
        key: CredentialKey,
        inject_identity: bool,
    ) -> Self {
        Self {
            base_url: BASE_URL.to_owned(),
            store,
            key,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_mins(5))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            inject_identity,
        }
    }
}

pub fn build(store: &CredentialStore, account: &str) -> AnthropicProvider {
    build_with_identity(store, account, true)
}

pub fn build_with_identity(
    store: &CredentialStore,
    account: &str,
    inject_identity: bool,
) -> AnthropicProvider {
    let key = CredentialKey::model(PROVIDER_ID, account);
    AnthropicProvider::with_identity(store.clone(), key, inject_identity)
}

fn build_system(system: &str, oauth: bool) -> Option<serde_json::Value> {
    if oauth {
        let mut blocks = vec![json!({ "type": "text", "text": CLAUDE_CODE_SYSTEM })];
        if !system.is_empty() {
            blocks.push(json!({ "type": "text", "text": system }));
        }
        Some(serde_json::Value::Array(blocks))
    } else if system.is_empty() {
        None
    } else {
        Some(serde_json::Value::Array(vec![json!({
            "type": "text",
            "text": system,
        })]))
    }
}

fn cache_marker() -> serde_json::Value {
    json!({ "type": "ephemeral" })
}

fn mark_last_cacheable_block(message: &mut serde_json::Value) -> bool {
    let Some(blocks) = message
        .get_mut("content")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return false;
    };
    for block in blocks.iter_mut().rev() {
        let kind = block
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if kind == "thinking" || kind == "redacted_thinking" {
            continue;
        }
        if let Some(object) = block.as_object_mut() {
            object.insert("cache_control".to_owned(), cache_marker());
            return true;
        }
    }
    false
}

fn apply_cache_control(
    system: &mut Option<serde_json::Value>,
    messages: &mut [serde_json::Value],
    tools: &mut [serde_json::Value],
) {
    if let Some(tool) = tools.last_mut()
        && let Some(object) = tool.as_object_mut()
    {
        object.insert("cache_control".to_owned(), cache_marker());
    }
    if let Some(serde_json::Value::Array(blocks)) = system
        && let Some(last) = blocks.last_mut()
        && let Some(object) = last.as_object_mut()
    {
        object.insert("cache_control".to_owned(), cache_marker());
    }
    let mut marked = 0;
    for message in messages.iter_mut().rev() {
        if marked == 2 {
            break;
        }
        if mark_last_cacheable_block(message) {
            marked += 1;
        }
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<serde_json::Value>,
    messages: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<serde_json::Value>,
}

struct ThinkingConfig {
    thinking: Option<serde_json::Value>,
    output_config: Option<serde_json::Value>,
    max_tokens: u32,
}

fn uses_effort_param(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    id.contains("fable")
        || id.contains("opus-5")
        || id.contains("opus-4-8")
        || id.contains("opus-4-7")
        || id.contains("opus-4-6")
        || id.contains("sonnet-4-6")
}

fn budget_tokens(effort: Effort) -> Option<u32> {
    match effort {
        Effort::Off => None,
        Effort::Low => Some(2048),
        Effort::Medium => Some(8192),
        Effort::High | Effort::Xhigh | Effort::Max => Some(24576),
    }
}

fn thinking_config(model: &str, effort: Option<Effort>) -> ThinkingConfig {
    let none = ThinkingConfig {
        thinking: None,
        output_config: None,
        max_tokens: MAX_TOKENS,
    };
    let Some(effort) = effort else {
        return none;
    };
    if uses_effort_param(model) {
        if matches!(effort, Effort::Off) {
            return none;
        }
        ThinkingConfig {
            thinking: Some(json!({ "type": "adaptive", "display": "summarized" })),
            output_config: Some(json!({ "effort": effort.as_str() })),
            max_tokens: MAX_TOKENS,
        }
    } else {
        match budget_tokens(effort) {
            None => none,
            Some(budget) => ThinkingConfig {
                thinking: Some(json!({ "type": "enabled", "budget_tokens": budget })),
                output_config: None,
                max_tokens: budget + MAX_TOKENS,
            },
        }
    }
}

const UNIFIED_PREFIX: &str = "anthropic-ratelimit-unified-";
const UNIFIED_SUFFIX: &str = "-utilization";

fn unified_label(period: &str) -> String {
    match period {
        "5h" => "5h".to_owned(),
        "7d" => "weekly".to_owned(),
        other => other.to_owned(),
    }
}

fn representative_period(claim: &str) -> String {
    match claim {
        "five_hour" => "5h".to_owned(),
        "seven_day" => "7d".to_owned(),
        other => other.replace('_', ""),
    }
}

fn parse_anthropic_unified_ratelimits(
    headers: &reqwest::header::HeaderMap,
) -> Option<RateLimitSnapshot> {
    let mut periods: Vec<String> = Vec::new();
    for name in headers.keys() {
        let key = name.as_str();
        if let Some(rest) = key.strip_prefix(UNIFIED_PREFIX)
            && let Some(period) = rest.strip_suffix(UNIFIED_SUFFIX)
            && !period.is_empty()
            && !periods.iter().any(|p| p == period)
        {
            periods.push(period.to_owned());
        }
    }
    periods.sort_by_key(|p| match p.as_str() {
        "5h" => 0,
        "7d" => 1,
        _ => 2,
    });

    let mut windows = Vec::new();
    for period in &periods {
        if let Some(window) = parse_unified_window(headers, period, &unified_label(period)) {
            windows.push(window);
        }
    }

    if windows.is_empty() {
        return None;
    }

    let representative = headers
        .get("anthropic-ratelimit-unified-representative-claim")
        .and_then(|v| v.to_str().ok())
        .map(|claim| unified_label(&representative_period(claim)));

    Some(RateLimitSnapshot {
        windows,
        representative,
    })
}

pub(crate) fn unified_reset_secs(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    let snapshot = parse_anthropic_unified_ratelimits(headers)?;
    let representative = snapshot.representative.as_ref()?;
    snapshot
        .windows
        .iter()
        .find(|window| &window.label == representative)
        .and_then(|window| window.resets_at)
}

fn parse_unified_window(
    headers: &reqwest::header::HeaderMap,
    period: &str,
    label: &str,
) -> Option<RateWindow> {
    let util_key = format!("anthropic-ratelimit-unified-{period}-utilization");
    let reset_key = format!("anthropic-ratelimit-unified-{period}-reset");

    let raw = headers.get(&util_key).and_then(|v| v.to_str().ok())?;
    let fraction: f32 = raw.trim_end_matches('%').parse().ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let used_percent = if raw.ends_with('%') {
        fraction
    } else {
        fraction * 100.0
    };

    let resets_at = headers
        .get(&reset_key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok().or_else(|| parse_rfc3339_unix(s)));

    Some(RateWindow {
        label: label.to_owned(),
        used_percent,
        resets_at,
    })
}

fn parse_rfc3339_unix(s: &str) -> Option<i64> {
    let s = if let Some(pos) = s.find('+') {
        &s[..pos]
    } else {
        s.trim_end_matches('Z')
    };
    let (date, time) = s.split_once('T')?;
    let mut dp = date.splitn(3, '-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: i64 = dp.next()?.parse().ok()?;
    let day: i64 = dp.next()?.parse().ok()?;
    let mut tp = time.splitn(3, ':');
    let hour: i64 = tp.next()?.parse().ok()?;
    let min: i64 = tp.next()?.parse().ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let sec: i64 = tp.next()?.trim_end_matches('Z').parse::<f64>().ok()? as i64;
    Some(gregorian_to_unix(year, month, day, hour, min, sec))
}

fn gregorian_to_unix(year: i64, month: i64, day: i64, h: i64, m: i64, s: i64) -> i64 {
    let (y, mo) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * mo + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + h * 3_600 + m * 60 + s
}

fn anthropic_efforts(model: &str) -> Vec<Effort> {
    let id = model.to_ascii_lowercase();
    if id.contains("fable")
        || id.contains("opus-5")
        || id.contains("opus-4-8")
        || id.contains("opus-4-7")
    {
        vec![
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ]
    } else if id.contains("opus-4-6") || id.contains("sonnet-5") || id.contains("sonnet-4-6") {
        vec![Effort::Low, Effort::Medium, Effort::High, Effort::Max]
    } else if id.contains("opus-4-5")
        || id.contains("sonnet-4-5")
        || id.contains("opus-4-1")
        || id.contains("haiku-4-5")
    {
        vec![Effort::Off, Effort::Low, Effort::Medium, Effort::High]
    } else {
        Vec::new()
    }
}

fn content_block_json(block: &ContentBlock) -> serde_json::Value {
    match block {
        ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
        ContentBlock::Thinking { text, signature } => json!({
            "type": "thinking",
            "thinking": text,
            "signature": signature,
        }),
        ContentBlock::RedactedThinking { data } => json!({
            "type": "redacted_thinking",
            "data": data,
        }),
        ContentBlock::ToolUse { id, name, input } => json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": if input.is_object() { input.clone() } else { json!({}) },
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let content_json: Vec<serde_json::Value> =
                content.iter().map(content_block_json).collect();
            json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content_json,
                "is_error": is_error,
            })
        }
        ContentBlock::Image { media_type, data } => json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
            },
        }),
    }
}

fn message_json(role: &str, message: &Message) -> serde_json::Value {
    let blocks: Vec<serde_json::Value> = message.content.iter().map(content_block_json).collect();
    json!({ "role": role, "content": blocks })
}

#[derive(Deserialize)]
struct DeltaEvent {
    delta: Option<DeltaBody>,
}

#[derive(Deserialize)]
struct DeltaBody {
    text: Option<String>,
    partial_json: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
}

#[allow(clippy::struct_field_names)]
#[derive(Default, Deserialize)]
struct MessageStartUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

#[derive(Deserialize)]
struct MessageStart {
    message: MessageStartMessage,
}

#[derive(Deserialize)]
struct MessageStartMessage {
    #[serde(default)]
    usage: MessageStartUsage,
}

#[derive(Default, Deserialize)]
struct MessageDeltaUsage {
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize)]
struct MessageDelta {
    usage: Option<MessageDeltaUsage>,
}

#[derive(Deserialize)]
struct ContentBlockStart {
    index: u32,
    content_block: ContentBlockInfo,
}

#[derive(Deserialize)]
struct ContentBlockInfo {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    name: Option<String>,
    data: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlockStop {
    index: u32,
}

#[derive(Deserialize)]
struct IndexedEvent {
    index: u32,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelDto>,
}

#[derive(Deserialize)]
struct ModelDto {
    id: String,
}

fn parse_text_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data).ok()?.delta?.text
}

fn parse_input_json_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data)
        .ok()?
        .delta?
        .partial_json
}

fn parse_thinking_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data)
        .ok()?
        .delta?
        .thinking
}

fn parse_signature_delta(data: &str) -> Option<String> {
    serde_json::from_str::<DeltaEvent>(data)
        .ok()?
        .delta?
        .signature
}

fn event_index(data: &str) -> Result<u32, serde_json::Error> {
    serde_json::from_str::<IndexedEvent>(data).map(|event| event.index)
}

fn split_request(req: &Request) -> (String, Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut system = String::new();
    let mut messages = Vec::new();
    for message in &req.messages {
        match message.role {
            MessageRole::System => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(&message.text_content());
            }
            MessageRole::User => messages.push(message_json("user", message)),
            MessageRole::Assistant => messages.push(message_json("assistant", message)),
        }
    }
    let tools = req
        .tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            })
        })
        .collect();
    (system, messages, tools)
}

#[allow(clippy::too_many_lines)]
fn stream_messages(
    response: reqwest::Response,
    rate_limits: Option<RateLimitSnapshot>,
) -> ChunkStream {
    Box::pin(async_stream::try_stream! {
        if let Some(snapshot) = rate_limits {
            yield StreamChunk::RateLimits { snapshot };
        }
        let mut stream = response.bytes_stream().eventsource();
        let mut tool_calls: HashMap<u32, (String, String, String)> = HashMap::new();
        let mut usage = Usage::default();
        let mut stopped = false;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => match event.event.as_str() {
                    "message_start" => {
                        if let Ok(start) = serde_json::from_str::<MessageStart>(&event.data) {
                            let u = &start.message.usage;
                            usage.input_tokens = u.input_tokens
                                + u.cache_creation_input_tokens
                                + u.cache_read_input_tokens;
                            usage.cache_read_tokens = u.cache_read_input_tokens;
                            usage.cache_write_tokens = u.cache_creation_input_tokens;
                        }
                    }
                    "message_delta" => {
                        if let Ok(delta) = serde_json::from_str::<MessageDelta>(&event.data)
                            && let Some(u) = delta.usage
                        {
                            usage.output_tokens = u.output_tokens;
                        }
                    }
                    "content_block_start" => {
                        if let Ok(start) = serde_json::from_str::<ContentBlockStart>(&event.data) {
                            match start.content_block.kind.as_str() {
                                "tool_use" => {
                                    if let (Some(id), Some(name)) =
                                        (start.content_block.id, start.content_block.name)
                                    {
                                        tool_calls.insert(start.index, (id, name, String::new()));
                                    }
                                }
                                "redacted_thinking" => {
                                    if let Some(data) = start.content_block.data {
                                        yield StreamChunk::RedactedThinking { data };
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "content_block_delta" => {
                        if let Some(text) = parse_text_delta(&event.data) {
                            yield StreamChunk::TextDelta { text };
                        } else if let Some(text) = parse_thinking_delta(&event.data) {
                            yield StreamChunk::ThinkingDelta { text };
                        } else if let Some(signature) = parse_signature_delta(&event.data) {
                            yield StreamChunk::ThinkingSignature { signature };
                        } else if let Some(partial) = parse_input_json_delta(&event.data)
                            && let Ok(index) = event_index(&event.data)
                            && let Some(entry) = tool_calls.get_mut(&index)
                        {
                            entry.2.push_str(&partial);
                        }
                    }
                    "content_block_stop" => {
                        if let Ok(stop) = serde_json::from_str::<ContentBlockStop>(&event.data)
                            && let Some((id, name, input)) = tool_calls.remove(&stop.index)
                        {
                            yield StreamChunk::ToolCall { id, name, input };
                        }
                    }
                    "message_stop" => {
                        yield StreamChunk::Usage { usage };
                        stopped = true;
                        break;
                    }
                    "error" => {
                        Err(error::classify_sse_error(&event.data))?;
                    }
                    _ => {}
                },
                Err(err) => {
                    Err(goat_provider::StreamError::transport(err.to_string()))?;
                }
            }
        }
        if !stopped {
            Err(goat_provider::StreamError::transport(
                "stream ended before message_stop".to_owned(),
            ))?;
        }
    })
}

fn parse_web_search_results(value: &serde_json::Value) -> Vec<SearchResult> {
    let mut out = Vec::new();
    let Some(content) = value.get("content").and_then(|content| content.as_array()) else {
        return out;
    };
    for block in content {
        if block.get("type").and_then(|kind| kind.as_str()) != Some("web_search_tool_result") {
            continue;
        }
        let Some(results) = block.get("content").and_then(|content| content.as_array()) else {
            continue;
        };
        for result in results {
            if result.get("type").and_then(|kind| kind.as_str()) != Some("web_search_result") {
                continue;
            }
            let url = result
                .get("url")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            out.push(SearchResult {
                title: result
                    .get("title")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                url: url.to_owned(),
                snippet: result
                    .get("page_age")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
    }
    out
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            auth: AuthMethod::ApiKeyOrOAuth,
            images: true,
        }
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            env_var: Some(ENV_VAR),
            validation: "network",
            endpoint: None,
            oauth: Some("browser"),
            login_endpoint: None,
            setup: &[],
        }
    }

    fn supports_images(&self, model: &str) -> bool {
        anthropic_supports_images(model)
    }

    fn supports_web_search(&self) -> bool {
        true
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let client = self.client.clone();
        let url = format!("{}/messages", self.base_url);
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(auth) = current_auth(&store, &key).await else {
                return Err(StreamError::auth("not logged in to anthropic"));
            };
            let body = json!({
                "model": WEB_SEARCH_MODEL,
                "max_tokens": 1024,
                "messages": [{ "role": "user", "content": query }],
                "tools": [{ "type": "web_search_20250305", "name": "web_search", "max_uses": 5 }],
            });
            let builder = client
                .post(&url)
                .header("anthropic-version", VERSION)
                .json(&body);
            let builder = match &auth {
                Auth::ApiKey(api_key) => builder.header("x-api-key", api_key),
                Auth::OAuth(access) => builder
                    .bearer_auth(access)
                    .header("anthropic-beta", OAUTH_BETA)
                    .header("user-agent", OAUTH_USER_AGENT)
                    .header("x-app", "cli"),
            };
            let resp = builder.send().await.map_err(|err| error::transport(&err))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let headers = resp.headers().clone();
                let detail = resp.text().await.unwrap_or_default();
                return Err(error::classify_http(status, &headers, &detail));
            }
            let value: serde_json::Value = resp
                .json()
                .await
                .map_err(|err| StreamError::other(format!("invalid search response: {err}")))?;
            Ok(WebSearchOutput::from_results(parse_web_search_results(
                &value,
            )))
        })
    }

    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError> {
        let client = self.client.clone();
        let url = format!("{}/messages", self.base_url);
        let store = self.store.clone();
        let key = self.key.clone();
        let inject_identity = self.inject_identity;
        let Some(auth) = current_auth(&store, &key).await else {
            return Err(goat_provider::StreamError::auth(
                "not logged in to anthropic",
            ));
        };
        let (mut system, mut messages, mut tools) = split_request(&req);
        if let Some(req_system) = req.system.as_deref()
            && !req_system.is_empty()
        {
            if system.is_empty() {
                system = req_system.to_owned();
            } else {
                system = format!("{req_system}\n{system}");
            }
        }
        let cfg = thinking_config(&req.model, req.effort);
        let oauth = matches!(auth, Auth::OAuth(_));
        let mut system_value = build_system(&system, oauth && inject_identity);
        apply_cache_control(&mut system_value, &mut messages, &mut tools);
        let body = MessagesRequest {
            model: &req.model,
            max_tokens: req.max_tokens.unwrap_or(cfg.max_tokens),
            stream: true,
            system: system_value,
            messages,
            tools,
            temperature: req.temperature,
            tool_choice: matches!(req.tool_choice, goat_provider::ToolChoice::None)
                .then(|| json!({ "type": "none" })),
            thinking: cfg.thinking,
            output_config: cfg.output_config,
        };
        let builder = client
            .post(&url)
            .header("anthropic-version", VERSION)
            .json(&body);
        let builder = match &auth {
            Auth::ApiKey(api_key) => builder.header("x-api-key", api_key),
            Auth::OAuth(access) => builder
                .bearer_auth(access)
                .header("anthropic-beta", OAUTH_BETA)
                .header("user-agent", OAUTH_USER_AGENT)
                .header("x-app", "cli"),
        };
        let resp = builder.send().await.map_err(|err| error::transport(&err))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let headers = resp.headers().clone();
            let detail = resp.text().await.unwrap_or_default();
            return Err(error::classify_http(status, &headers, &detail));
        }
        let rate_limits = (matches!(auth, Auth::OAuth(_)))
            .then(|| parse_anthropic_unified_ratelimits(resp.headers()))
            .flatten();
        Ok(stream_messages(resp, rate_limits))
    }

    fn authenticated(&self) -> bool {
        self.store.resolve(&self.key, Some(ENV_VAR)).is_some()
    }

    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let auth = current_auth(&store, &key)
                .await
                .ok_or(ValidateError::NoCredentials)?;
            let api_key = match auth {
                Auth::OAuth(_) => return Ok(Validated::Assumed),
                Auth::ApiKey(api_key) => api_key,
            };
            let resp = client
                .get(&url)
                .header("anthropic-version", VERSION)
                .header("x-api-key", api_key)
                .send()
                .await
                .map_err(|_| ValidateError::unreachable("could not reach provider"))?;
            let status = resp.status();
            if status.is_success() {
                Ok(Validated::Live)
            } else if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                Err(ValidateError::InvalidCredentials)
            } else {
                Err(ValidateError::unreachable(format!(
                    "could not reach provider: {status}"
                )))
            }
        })
    }

    fn context_window(&self, model: &str) -> Option<u32> {
        Some(anthropic_context_window(model))
    }

    fn catalog(&self) -> &'static [&'static str] {
        CATALOG
    }

    fn efforts(&self, model: &str) -> Vec<Effort> {
        anthropic_efforts(model)
    }

    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = format!("{}/models", self.base_url);
        let store = self.store.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let Some(auth) = current_auth(&store, &key).await else {
                return;
            };
            let api_key = match auth {
                Auth::OAuth(_) => {
                    for &id in CATALOG {
                        if out
                            .send(Model {
                                id: id.to_owned(),
                                supports_images: anthropic_supports_images(id),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    return;
                }
                Auth::ApiKey(api_key) => api_key,
            };
            let Ok(resp) = client
                .get(&url)
                .header("anthropic-version", VERSION)
                .header("x-api-key", api_key)
                .send()
                .await
            else {
                return;
            };
            let Ok(models) = resp.json::<ModelsResponse>().await else {
                return;
            };
            for model in models.data {
                let supports_images = anthropic_supports_images(&model.id);
                if out
                    .send(Model {
                        id: model.id,
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

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        tokio::spawn(async move { do_login(&status).await.map_err(|e| e.to_string()) })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    use super::{
        event_index, parse_anthropic_unified_ratelimits, parse_input_json_delta, parse_text_delta,
        parse_web_search_results,
    };

    fn headers_from(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

    #[test]
    fn unified_scan_reads_5h_and_weekly_decimal() {
        let h = headers_from(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.018"),
            ("anthropic-ratelimit-unified-5h-reset", "1764554400"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.737"),
            ("anthropic-ratelimit-unified-7d-reset", "1764615600"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "five_hour",
            ),
        ]);
        let snap = parse_anthropic_unified_ratelimits(&h).expect("snapshot");
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[0].label, "5h");
        assert_eq!(snap.windows[1].label, "weekly");
        assert!((snap.windows[1].used_percent - 73.7).abs() < 0.1);
        assert_eq!(snap.representative.as_deref(), Some("5h"));
    }

    #[test]
    fn unified_scan_accepts_percent_encoding() {
        let h = headers_from(&[("anthropic-ratelimit-unified-5h-utilization", "42%")]);
        let snap = parse_anthropic_unified_ratelimits(&h).expect("snapshot");
        assert!((snap.windows[0].used_percent - 42.0).abs() < 0.1);
    }

    #[test]
    fn unified_scan_surfaces_unknown_period() {
        let h = headers_from(&[("anthropic-ratelimit-unified-30d-utilization", "0.5")]);
        let snap = parse_anthropic_unified_ratelimits(&h).expect("snapshot");
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].label, "30d");
    }

    #[test]
    fn unified_scan_ignores_non_period_headers() {
        let h = headers_from(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-fallback-percentage", "0.2"),
        ]);
        assert!(parse_anthropic_unified_ratelimits(&h).is_none());
    }

    #[test]
    fn extracts_web_search_results() {
        let value = serde_json::json!({
            "content": [
                { "type": "text", "text": "here are results" },
                { "type": "web_search_tool_result", "content": [
                    { "type": "web_search_result", "url": "https://a.example", "title": "A", "page_age": "today" },
                    { "type": "web_search_result", "url": "https://b.example", "title": "B" }
                ]}
            ]
        });
        let results = parse_web_search_results(&value);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://a.example");
        assert_eq!(results[0].title, "A");
        assert_eq!(results[0].snippet, "today");
        assert_eq!(results[1].url, "https://b.example");
    }

    #[test]
    fn ignores_search_errors() {
        let value = serde_json::json!({
            "content": [
                { "type": "web_search_tool_result", "content": {
                    "type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"
                }}
            ]
        });
        assert!(parse_web_search_results(&value).is_empty());
    }

    #[test]
    fn parses_text_delta() {
        let data = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}"#;
        assert_eq!(parse_text_delta(data).as_deref(), Some("Hi"));
    }

    #[test]
    fn authorize_url_carries_pkce_and_scope() {
        let url =
            crate::auth::authorize_url("CHAL", "STATE", "http://localhost:1234/callback").unwrap();
        assert!(url.contains("client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=STATE"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("user%3Ainference"));
        assert!(url.contains("org%3Acreate_api_key"));
        assert!(url.contains("user%3Afile_upload"));
        assert!(url.contains("localhost%3A1234%2Fcallback"));
    }

    #[test]
    fn oauth_system_prepends_claude_code_identity() {
        let blocks = super::build_system("be helpful", true).unwrap();
        assert_eq!(blocks[0]["text"], super::CLAUDE_CODE_SYSTEM);
        assert_eq!(blocks[1]["text"], "be helpful");
        let plain = super::build_system("be helpful", false).unwrap();
        assert_eq!(plain[0]["text"], "be helpful");
        assert!(super::build_system("", false).is_none());
    }

    #[test]
    fn cache_control_marks_tools_system_and_last_two_messages() {
        let mut system = super::build_system("be helpful", false);
        let mut tools = vec![
            serde_json::json!({ "name": "Read" }),
            serde_json::json!({ "name": "Bash" }),
        ];
        let mut messages = vec![
            serde_json::json!({ "role": "user", "content": [{ "type": "text", "text": "one" }] }),
            serde_json::json!({ "role": "assistant", "content": [
                { "type": "text", "text": "two" },
                { "type": "thinking", "thinking": "t", "signature": "s" },
            ] }),
            serde_json::json!({ "role": "user", "content": [{ "type": "text", "text": "three" }] }),
        ];
        super::apply_cache_control(&mut system, &mut messages, &mut tools);
        assert!(tools[0].get("cache_control").is_none());
        assert_eq!(tools[1]["cache_control"]["type"], "ephemeral");
        let system = system.unwrap();
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
        assert!(messages[0]["content"][0].get("cache_control").is_none());
        assert_eq!(
            messages[1]["content"][0]["cache_control"]["type"], "ephemeral",
            "thinking blocks must be skipped when marking"
        );
        assert!(messages[1]["content"][1].get("cache_control").is_none());
        assert_eq!(
            messages[2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn text_delta_helper_skips_input_json() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\""}}"#;
        assert_eq!(parse_text_delta(data), None);
    }

    #[test]
    fn parses_input_json_delta() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\""}}"#;
        assert_eq!(parse_input_json_delta(data).as_deref(), Some("{\"a\""));
        assert_eq!(event_index(data).unwrap(), 1);
    }

    #[test]
    fn accumulates_tool_call_across_events() {
        let mut tool_calls: HashMap<u32, (String, String, String)> = HashMap::new();
        let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#;
        let start: super::ContentBlockStart = serde_json::from_str(start).unwrap();
        assert_eq!(start.content_block.kind, "tool_use");
        tool_calls.insert(
            start.index,
            (
                start.content_block.id.unwrap(),
                start.content_block.name.unwrap(),
                String::new(),
            ),
        );

        let first = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
        let second = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#;
        for chunk in [first, second] {
            let partial = parse_input_json_delta(chunk).unwrap();
            let index = event_index(chunk).unwrap();
            tool_calls.get_mut(&index).unwrap().2.push_str(&partial);
        }

        let stop = r#"{"type":"content_block_stop","index":0}"#;
        let stop: super::ContentBlockStop = serde_json::from_str(stop).unwrap();
        let (id, name, input) = tool_calls.remove(&stop.index).unwrap();
        assert_eq!(id, "toolu_1");
        assert_eq!(name, "read_file");
        assert_eq!(input, r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn handles_malformed_json() {
        assert_eq!(parse_text_delta("not json"), None);
        assert_eq!(parse_input_json_delta("not json"), None);
    }

    #[test]
    fn serializes_thinking_blocks() {
        use goat_provider::ContentBlock;
        let thinking = super::content_block_json(&ContentBlock::Thinking {
            text: "ponder".to_owned(),
            signature: "sig".to_owned(),
        });
        assert_eq!(thinking["type"], "thinking");
        assert_eq!(thinking["thinking"], "ponder");
        assert_eq!(thinking["signature"], "sig");
        let redacted = super::content_block_json(&ContentBlock::RedactedThinking {
            data: "blob".to_owned(),
        });
        assert_eq!(redacted["type"], "redacted_thinking");
        assert_eq!(redacted["data"], "blob");
    }

    #[test]
    fn effort_model_uses_output_config() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-opus-4-8", Some(Effort::High));
        let thinking = cfg.thinking.unwrap();
        assert_eq!(thinking["type"], "adaptive");
        assert_eq!(thinking["display"], "summarized");
        assert_eq!(cfg.output_config.unwrap()["effort"], "high");
    }

    #[test]
    fn fable_uses_output_config_with_xhigh() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-fable-5", Some(Effort::Xhigh));
        assert_eq!(cfg.thinking.unwrap()["type"], "adaptive");
        assert_eq!(cfg.output_config.unwrap()["effort"], "xhigh");
        assert_eq!(super::anthropic_context_window("claude-fable-5"), 1_000_000);
        assert!(super::anthropic_efforts("claude-fable-5").contains(&Effort::Xhigh));
        let off = super::thinking_config("claude-fable-5", Some(Effort::Off));
        assert!(off.thinking.is_none());
        assert!(off.output_config.is_none());
    }

    #[test]
    fn opus_5_uses_output_config_with_xhigh() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-opus-5", Some(Effort::Xhigh));
        assert_eq!(cfg.thinking.unwrap()["type"], "adaptive");
        assert_eq!(cfg.output_config.unwrap()["effort"], "xhigh");
        assert_eq!(super::anthropic_context_window("claude-opus-5"), 1_000_000);
        assert!(super::anthropic_efforts("claude-opus-5").contains(&Effort::Xhigh));
        assert!(super::CATALOG.contains(&"claude-opus-5"));
    }

    #[test]
    fn opus_5_does_not_match_opus_4_5() {
        use goat_provider::Effort;
        assert!(!super::uses_effort_param("claude-opus-4-5-20251101"));
        assert_eq!(
            super::anthropic_context_window("claude-opus-4-5-20251101"),
            200_000
        );
        assert!(super::anthropic_efforts("claude-opus-4-5-20251101").contains(&Effort::Off));
    }

    #[test]
    fn budget_model_uses_budget_tokens() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-sonnet-4-5-20250929", Some(Effort::Medium));
        assert_eq!(cfg.thinking.as_ref().unwrap()["type"], "enabled");
        assert_eq!(cfg.thinking.unwrap()["budget_tokens"], 8192);
        assert!(cfg.max_tokens > 8192);
        assert!(cfg.output_config.is_none());
    }

    #[test]
    fn off_disables_thinking() {
        use goat_provider::Effort;
        let cfg = super::thinking_config("claude-sonnet-4-5-20250929", Some(Effort::Off));
        assert!(cfg.thinking.is_none());
        let none = super::thinking_config("claude-opus-4-8", None);
        assert!(none.thinking.is_none());
    }

    #[test]
    fn no_effort_disables_thinking() {
        let none = super::thinking_config("claude-opus-4-8", None);
        assert!(none.thinking.is_none());
        assert!(none.output_config.is_none());
    }
}

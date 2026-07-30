use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use tokio::{sync::mpsc, task::JoinHandle};

pub use goat_auth::{TokenSet, now_secs};
pub use goat_protocol::{AuthMethod, Effort, RateLimitSnapshot, RateWindow, Usage};

use std::fmt;
use std::fmt::Write as _;

fn deser_tool_result_content<'de, D>(d: D) -> Result<Vec<ContentBlock>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::String(s) => Ok(vec![ContentBlock::Text { text: s }]),
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(D::Error::custom))
            .collect(),
        other => Err(D::Error::custom(format!(
            "expected string or array for tool_result content, got {other}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(deserialize_with = "deser_tool_result_content")]
        content: Vec<ContentBlock>,
        is_error: bool,
    },
    Image {
        media_type: String,
        data: String,
    },
}

impl ContentBlock {
    pub fn text_result(
        tool_use_id: impl Into<String>,
        text: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: vec![Self::Text { text: text.into() }],
            is_error,
        }
    }

    pub fn tool_result_text(content: &[ContentBlock]) -> String {
        content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub supports_images: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub effort: Option<Effort>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub tools: bool,
    pub auth: AuthMethod,
    pub images: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchOutput {
    pub content: String,
    pub results: Vec<SearchResult>,
}

impl WebSearchOutput {
    #[must_use]
    pub fn from_results(results: Vec<SearchResult>) -> Self {
        Self {
            content: format_search_results(&results),
            results,
        }
    }
}

#[must_use]
pub fn format_search_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results found.".to_owned();
    }
    let mut out = String::new();
    for (index, result) in results.iter().enumerate() {
        let title = if result.title.is_empty() {
            &result.url
        } else {
            &result.title
        };
        let _ = write!(out, "{}. {title}\n   {}", index + 1, result.url);
        if !result.snippet.is_empty() {
            let _ = write!(out, " · {}", result.snippet);
        }
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ThinkingSignature {
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: String,
    },
    Usage {
        usage: Usage,
    },
    RateLimits {
        snapshot: RateLimitSnapshot,
    },
}

pub type ChunkStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamChunk, StreamError>> + Send>>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
pub enum StreamError {
    #[error("rate limited: {message}")]
    RateLimited {
        retry_after: Option<std::time::Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resets_at: Option<i64>,
        message: String,
    },
    #[error("provider overloaded: {message}")]
    Overloaded { message: String },
    #[error("context window exceeded: {message}")]
    ContextOverflow { message: String },
    #[error("authentication failed: {message}")]
    Auth { message: String },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("connection failed: {message}")]
    Transport { message: String },
    #[error("{message}")]
    Other { message: String },
}

impl StreamError {
    pub fn rate_limited(
        message: impl Into<String>,
        retry_after: Option<std::time::Duration>,
    ) -> Self {
        Self::RateLimited {
            retry_after,
            resets_at: None,
            message: message.into(),
        }
    }

    pub fn rate_limited_at(
        message: impl Into<String>,
        retry_after: Option<std::time::Duration>,
        resets_at: Option<i64>,
    ) -> Self {
        Self::RateLimited {
            retry_after,
            resets_at,
            message: message.into(),
        }
    }

    pub fn overloaded(message: impl Into<String>) -> Self {
        Self::Overloaded {
            message: message.into(),
        }
    }

    pub fn context_overflow(message: impl Into<String>) -> Self {
        Self::ContextOverflow {
            message: message.into(),
        }
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth {
            message: message.into(),
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::InvalidRequest {
            message: message.into(),
        }
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidateError {
    InvalidCredentials,
    NoCredentials,
    Unreachable(String),
}

impl ValidateError {
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::Unreachable(message.into())
    }
}

impl fmt::Display for ValidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCredentials => f.write_str("invalid credentials"),
            Self::NoCredentials => f.write_str("no credentials"),
            Self::Unreachable(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for ValidateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validated {
    Live,
    Assumed,
}

pub type EndpointValidator = fn(&str) -> Result<String, String>;

#[derive(Debug, Clone, Copy)]
pub struct LoginEndpointMetadata {
    pub env_var: Option<&'static str>,
    pub default: Option<&'static str>,
    pub validate: Option<EndpointValidator>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderMetadata {
    pub env_var: Option<&'static str>,
    pub validation: &'static str,
    pub endpoint: Option<&'static str>,
    pub oauth: Option<&'static str>,
    pub login_endpoint: Option<LoginEndpointMetadata>,
    pub setup: &'static [&'static str],
}

impl ProviderMetadata {
    pub const fn default() -> Self {
        Self {
            env_var: None,
            validation: "network",
            endpoint: None,
            oauth: None,
            login_endpoint: None,
            setup: &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelListSource {
    Catalog,
    Discover,
}

#[async_trait]
pub trait Provider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> Capabilities;
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata::default()
    }
    async fn stream(&self, req: Request) -> Result<ChunkStream, StreamError>;
    fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()>;

    fn model_list_source(&self) -> ModelListSource {
        if self.list_models().is_empty() {
            ModelListSource::Discover
        } else {
            ModelListSource::Catalog
        }
    }

    fn list_models(&self) -> Vec<String> {
        Vec::new()
    }

    fn efforts(&self, _model: &str) -> Vec<Effort> {
        Vec::new()
    }
    fn authenticated(&self) -> bool {
        true
    }
    fn validate(&self) -> JoinHandle<Result<Validated, ValidateError>> {
        tokio::spawn(async { Ok(Validated::Assumed) })
    }

    fn verifies_credentials(&self) -> bool {
        true
    }

    fn context_window(&self, _model: &str) -> Option<u32> {
        None
    }

    fn supports_images(&self, _model: &str) -> bool {
        self.capabilities().images
    }

    fn supports_web_search(&self) -> bool {
        false
    }

    fn web_search(&self, query: String) -> JoinHandle<Result<WebSearchOutput, StreamError>> {
        let _ = query;
        tokio::spawn(async { Err(StreamError::other("web search is not supported")) })
    }

    fn login(&self, status: mpsc::Sender<String>) -> JoinHandle<Result<TokenSet, String>> {
        let _ = status;
        tokio::spawn(async { Err("login not supported".into()) })
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use tokio::{sync::mpsc, task::JoinHandle};

    use super::{
        AuthMethod, Capabilities, ChunkStream, Message, MessageRole, Model, Provider, ProviderId,
        Request, StreamChunk,
    };

    struct MockProvider;

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tools: false,
                auth: AuthMethod::None,
                images: false,
            }
        }

        async fn stream(&self, _req: Request) -> Result<ChunkStream, super::StreamError> {
            Ok(Box::pin(async_stream::try_stream! {
                yield StreamChunk::TextDelta { text: "hi".into() };
            }))
        }

        fn discover(&self, out: mpsc::Sender<Model>) -> JoinHandle<()> {
            tokio::spawn(async move {
                let _ = out
                    .send(Model {
                        id: "mock-1".into(),
                        supports_images: false,
                    })
                    .await;
            })
        }
    }

    #[tokio::test]
    async fn mock_provider_streams_events() {
        let provider = MockProvider;
        assert_eq!(provider.id(), ProviderId::from("mock"));
        assert!(!provider.capabilities().tools);
        let mut stream = provider
            .stream(Request {
                model: "mock-1".into(),
                messages: vec![Message::text(MessageRole::User, "hi")],
                tools: vec![],
                effort: None,
                tool_choice: super::ToolChoice::Auto,
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
        assert_eq!(chunks.len(), 1);
        assert!(matches!(
            &chunks[0],
            StreamChunk::TextDelta { text } if text == "hi"
        ));
    }

    #[tokio::test]
    async fn mock_provider_discovers_models() {
        let provider = MockProvider;
        let (tx, mut rx) = mpsc::channel(8);
        let handle = provider.discover(tx);
        let info = rx.recv().await.unwrap();
        assert_eq!(info.id, "mock-1");
        handle.await.unwrap();
    }

    #[test]
    fn content_blocks_round_trip_through_json() {
        use super::ContentBlock;
        let blocks = vec![
            ContentBlock::Text {
                text: "hello".into(),
            },
            ContentBlock::Thinking {
                text: "step one".into(),
                signature: "sig-abc".into(),
            },
            ContentBlock::RedactedThinking {
                data: "opaque".into(),
            },
            ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "Read".into(),
                input: serde_json::json!({"path": "src/lib.rs"}),
            },
            ContentBlock::ToolResult {
                tool_use_id: "call-1".into(),
                content: vec![ContentBlock::Text {
                    text: "result".into(),
                }],
                is_error: false,
            },
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "base64data".into(),
            },
        ];
        let json = serde_json::to_string(&blocks).unwrap();
        let restored: Vec<ContentBlock> = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, blocks);
    }
}

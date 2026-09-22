use std::{
    any::Any,
    borrow::Cow,
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};

use goat_protocol::{TaskId, ToolCallId, ToolDisplay, ToolOutcome, TranscriptEntry};
use goat_types::{AgentId, ConversationId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{context::ToolSandbox, display, error::ToolError, name::ToolName, spec::ToolSpec};

pub struct ToolCall {
    pub call_id: String,
    pub name: ToolName,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolContent {
    Text { text: String },
    Image { media_type: String, data: String },
}

pub struct ToolImage {
    pub media_type: String,
    pub data: String,
}

pub trait ToolOutcomeExtension: Send + Sync {
    fn apply(&self, outcome: &mut ToolOutcome);
}

pub struct ToolOutput {
    pub content: Vec<ToolContent>,
    pub structured_content: Option<serde_json::Value>,
    pub is_error: bool,
    pub summary: Option<String>,
    pub body: Option<String>,
    extensions: Vec<Box<dyn ToolOutcomeExtension>>,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: s.into() }],
            structured_content: None,
            is_error: false,
            summary: None,
            body: None,
            extensions: Vec::new(),
        }
    }

    pub fn png(data: impl Into<String>) -> Self {
        Self::image(ToolImage {
            media_type: "image/png".to_owned(),
            data: data.into(),
        })
    }

    pub fn image(image: ToolImage) -> Self {
        Self {
            content: vec![ToolContent::Image {
                media_type: image.media_type,
                data: image.data,
            }],
            structured_content: None,
            is_error: false,
            summary: None,
            body: None,
            extensions: Vec::new(),
        }
    }

    pub fn image_with_text(text: impl Into<String>, image: ToolImage) -> Self {
        Self {
            content: vec![
                ToolContent::Text { text: text.into() },
                ToolContent::Image {
                    media_type: image.media_type,
                    data: image.data,
                },
            ],
            structured_content: None,
            is_error: false,
            summary: None,
            body: None,
            extensions: Vec::new(),
        }
    }

    pub fn structured(value: serde_json::Value) -> Self {
        Self {
            content: vec![ToolContent::Text {
                text: value.to_string(),
            }],
            structured_content: Some(value),
            is_error: false,
            summary: None,
            body: None,
            extensions: Vec::new(),
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: text.into() }],
            structured_content: None,
            is_error: true,
            summary: None,
            body: None,
            extensions: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    #[must_use]
    pub fn with_extension(mut self, extension: impl ToolOutcomeExtension + 'static) -> Self {
        self.extensions.push(Box::new(extension));
        self
    }

    pub fn extend_outcome(&self, outcome: &mut ToolOutcome) {
        for extension in &self.extensions {
            extension.apply(outcome);
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        self.content.iter().find_map(|part| match part {
            ToolContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
    }

    pub fn text_for_model(&self) -> String {
        let mut out = String::new();
        for part in &self.content {
            if let ToolContent::Text { text } = part {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        if out.is_empty()
            && let Some(value) = &self.structured_content
        {
            return value.to_string();
        }
        out
    }
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>>;
pub type ToolBatchFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub struct ToolBatchCall<'a> {
    pub call: ToolCallId,
    pub input: &'a str,
}

pub struct ToolBatchInvocation {
    pub task: TaskId,
}

pub trait ToolHistoryGroup: Send + Sync {
    fn entry(&self, outcomes: Vec<ToolOutcome>) -> TranscriptEntry;
}

#[derive(Clone, Copy, Default)]
pub struct ToolDefinitionContext {
    pub interactive: bool,
    pub top_level: bool,
    pub planning: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ToolSummaryKind {
    Summary,
    Body,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolAudience {
    Principal(String),
    Shared(String),
}

pub type ToolReadState = Arc<Mutex<HashMap<PathBuf, ToolReadSnapshot>>>;

#[derive(Clone, Debug)]
pub struct ToolReadSnapshot {
    pub size: u64,
    pub modified_ms: Option<u128>,
    pub hash: u64,
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub struct AgentContext {
    pub id: AgentId,
    pub slug: String,
    pub conversation: ConversationId,
    pub audience: Option<ToolAudience>,
    pub read_state: ToolReadState,
}

#[derive(Clone, Copy)]
pub struct ToolContext<'a> {
    pub sandbox: &'a ToolSandbox,
    pub cancellation: &'a CancellationToken,
    pub definition_context: ToolDefinitionContext,
    pub host: Option<&'a (dyn Any + Send + Sync)>,
    pub task: Option<TaskId>,
    pub call: Option<ToolCallId>,
    pub agent: Option<&'a AgentContext>,
}

impl<'a> ToolContext<'a> {
    pub fn new(sandbox: &'a ToolSandbox, cancellation: &'a CancellationToken) -> Self {
        Self {
            sandbox,
            cancellation,
            definition_context: ToolDefinitionContext::default(),
            host: None,
            task: None,
            call: None,
            agent: None,
        }
    }

    pub fn agent(
        sandbox: &'a ToolSandbox,
        cancellation: &'a CancellationToken,
        agent: &'a AgentContext,
    ) -> Self {
        Self {
            agent: Some(agent),
            ..Self::new(sandbox, cancellation)
        }
    }

    pub fn resolve(&self, raw: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        self.sandbox.resolve(raw)
    }

    pub fn agent_context(&self) -> Result<&AgentContext, ToolError> {
        self.agent
            .ok_or_else(|| ToolError::execution("tool requires an agent context"))
    }
}

impl std::ops::Deref for ToolContext<'_> {
    type Target = ToolSandbox;

    fn deref(&self) -> &ToolSandbox {
        self.sandbox
    }
}

fn never_cancelled() -> &'static CancellationToken {
    static TOKEN: std::sync::OnceLock<CancellationToken> = std::sync::OnceLock::new();
    TOKEN.get_or_init(CancellationToken::new)
}

pub trait Tool: Send + Sync {
    fn name(&self) -> ToolName;
    fn description(&self) -> Cow<'static, str>;
    fn parameters(&self) -> serde_json::Value;
    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a>;
    fn run<'a>(&'a self, input: &'a str, sandbox: &'a ToolSandbox) -> ToolFuture<'a> {
        let name = self.name();
        let parsed = serde_json::from_str(input);
        Box::pin(async move {
            let arguments = parsed.map_err(|source| {
                ToolError::invalid_input(format!("invalid tool input: {source}"))
            })?;
            let call = ToolCall {
                call_id: String::new(),
                name,
                arguments,
            };
            self.call(&call, ToolContext::new(sandbox, never_cancelled()))
                .await
        })
    }
    fn enabled(&self, _context: ToolDefinitionContext) -> bool {
        true
    }
    fn default_enabled(&self) -> bool {
        true
    }
    fn definition(&self, context: ToolDefinitionContext) -> Option<ToolSpec> {
        self.enabled(context)
            .then(|| ToolSpec::new(self.name(), self.description(), self.parameters()))
    }
    fn handles_cancellation(&self) -> bool {
        false
    }
    fn batch_started<'a>(
        &'a self,
        _calls: &'a [ToolBatchCall<'a>],
        _invocation: ToolBatchInvocation,
    ) -> ToolBatchFuture<'a> {
        Box::pin(async {})
    }
    fn history_group(&self, _calls: &[ToolBatchCall<'_>]) -> Option<Box<dyn ToolHistoryGroup>> {
        None
    }
    fn summary_kind(&self) -> ToolSummaryKind {
        ToolSummaryKind::Summary
    }
    fn mutation_path(&self, _input: &str) -> Option<String> {
        None
    }
    fn display_input(&self, input: &str) -> ToolDisplay {
        display::generic(input)
    }
}

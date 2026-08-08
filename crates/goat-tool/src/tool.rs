use std::{any::Any, future::Future, pin::Pin};

use goat_protocol::{TaskId, ToolCallId, ToolDisplay, ToolOutcome, TranscriptEntry};
use tokio_util::sync::CancellationToken;

use crate::{context::ToolContext, display, error::ToolError};

pub struct ToolImage {
    pub media_type: String,
    pub data: String,
}

pub enum ToolContent {
    Text(String),
    Image(ToolImage),
}

pub trait ToolOutcomeExtension: Send + Sync {
    fn apply(&self, outcome: &mut ToolOutcome);
}

pub struct ToolOutput {
    pub content: ToolContent,
    pub summary: Option<String>,
    pub body: Option<String>,
    extensions: Vec<Box<dyn ToolOutcomeExtension>>,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: ToolContent::Text(s.into()),
            summary: None,
            body: None,
            extensions: Vec::new(),
        }
    }

    pub fn png(data: impl Into<String>) -> Self {
        Self {
            content: ToolContent::Image(ToolImage {
                media_type: "image/png".to_owned(),
                data: data.into(),
            }),
            summary: None,
            body: None,
            extensions: Vec::new(),
        }
    }

    pub fn image(image: ToolImage) -> Self {
        Self {
            content: ToolContent::Image(image),
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
        match &self.content {
            ToolContent::Text(s) => Some(s),
            ToolContent::Image(_) => None,
        }
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

pub struct ToolInvocation<'a> {
    pub task: TaskId,
    pub call: ToolCallId,
    pub cancellation: &'a CancellationToken,
    pub definition_context: ToolDefinitionContext,
    pub host: Option<&'a (dyn Any + Send + Sync)>,
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> serde_json::Value;
    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a>;
    fn invoke<'a>(
        &'a self,
        input: &'a str,
        ctx: &'a ToolContext,
        _invocation: ToolInvocation<'a>,
    ) -> ToolFuture<'a> {
        self.run(input, ctx)
    }
    fn enabled(&self, _context: ToolDefinitionContext) -> bool {
        true
    }
    fn definition(&self, context: ToolDefinitionContext) -> Option<crate::ToolSpec> {
        self.enabled(context).then(|| crate::ToolSpec {
            name: self.name(),
            description: self.description().to_owned(),
            parameters: self.parameters(),
        })
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

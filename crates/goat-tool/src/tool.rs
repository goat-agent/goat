use std::{future::Future, pin::Pin};

use goat_protocol::ToolDisplay;

use crate::{context::ToolContext, display, error::ToolError};

pub struct ToolImage {
    pub media_type: String,
    pub data: String,
}

pub enum ToolContent {
    Text(String),
    Image(ToolImage),
}

pub struct ToolOutput {
    pub content: ToolContent,
    pub summary: Option<String>,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: ToolContent::Text(s.into()),
            summary: None,
        }
    }

    pub fn png(data: impl Into<String>) -> Self {
        Self {
            content: ToolContent::Image(ToolImage {
                media_type: "image/png".to_owned(),
                data: data.into(),
            }),
            summary: None,
        }
    }

    pub fn image(image: ToolImage) -> Self {
        Self {
            content: ToolContent::Image(image),
            summary: None,
        }
    }

    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn as_text(&self) -> Option<&str> {
        match &self.content {
            ToolContent::Text(s) => Some(s),
            ToolContent::Image(_) => None,
        }
    }
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>>;

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> serde_json::Value;
    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a>;
    fn display_input(&self, input: &str) -> ToolDisplay {
        display::generic(input)
    }
}

use std::sync::Arc;

use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolError, ToolFuture, ToolSandbox, display};

use crate::action::{self, Action, BrowserRef};
use crate::browser::{self, Browser};
use crate::transport::Transport;

pub struct BrowserTool {
    browser: Browser,
}

impl BrowserTool {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            browser: Browser::new(transport),
        }
    }
}

fn exec_err(err: impl std::fmt::Display) -> ToolError {
    ToolError::execution(err.to_string())
}

fn ref_label(reference: &BrowserRef) -> String {
    match &reference.snapshot_id {
        Some(snapshot_id) => format!("{snapshot_id}:{}", reference.reference),
        None => reference.reference.clone(),
    }
}

impl Tool for BrowserTool {
    fn name(&self) -> &'static str {
        browser::NAME
    }

    fn description(&self) -> &'static str {
        browser::DESCRIPTION
    }

    fn parameters(&self) -> serde_json::Value {
        browser::parameters()
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        let Ok(action) = action::parse(input) else {
            return display::generic(input);
        };
        match action {
            Action::Navigate { url } => ToolDisplay::with_detail("navigate", url),
            Action::Snapshot => ToolDisplay::primary("snapshot"),
            Action::Click { reference } => ToolDisplay::with_detail("click", ref_label(&reference)),
            Action::Fill {
                reference, text, ..
            } => ToolDisplay::with_detail(
                "fill",
                format!("{} · {}", ref_label(&reference), display::flatten(&text)),
            ),
            Action::Select { reference, value } => {
                ToolDisplay::with_detail("select", format!("{} · {value}", ref_label(&reference)))
            }
            Action::Hover { reference } => ToolDisplay::with_detail("hover", ref_label(&reference)),
            Action::Drag { from, to } => ToolDisplay::with_detail(
                "drag",
                format!("{} -> {}", ref_label(&from), ref_label(&to)),
            ),
            Action::Upload { reference, path } => {
                ToolDisplay::with_detail("upload", format!("{} · {path}", ref_label(&reference)))
            }
            Action::PressKey { key } => ToolDisplay::with_detail("press key", key),
            Action::Scroll { direction, amount } => {
                ToolDisplay::with_detail("scroll", format!("{direction:?} {amount:?}"))
            }
            Action::GoBack => ToolDisplay::primary("go back"),
            Action::GoForward => ToolDisplay::primary("go forward"),
            Action::FindText { query, .. } => ToolDisplay::with_detail("find text", query),
            Action::Inspect { reference, .. } => {
                ToolDisplay::with_detail("inspect", ref_label(&reference))
            }
            Action::ReadViewport { .. } => ToolDisplay::primary("read viewport"),
            Action::ReadContent { .. } => ToolDisplay::primary("read content"),
            Action::ReadNetwork { filter, .. } => {
                ToolDisplay::with_detail("read network", filter.unwrap_or_default())
            }
            Action::ReadConsole { level, .. } => {
                ToolDisplay::with_detail("read console", level.unwrap_or_default())
            }
            Action::Storage { op, .. } => ToolDisplay::with_detail("storage", format!("{op:?}")),
            Action::Tab { op, .. } => ToolDisplay::with_detail("tab", format!("{op:?}")),
            Action::WaitFor { text, state, .. } => ToolDisplay::with_detail(
                "wait for",
                text.or(state).unwrap_or_else(|| "condition".to_owned()),
            ),
            Action::Screenshot => ToolDisplay::primary("screenshot"),
            Action::DebugEval { js } => {
                ToolDisplay::with_detail("debug eval", display::flatten(&js))
            }
            Action::Close => ToolDisplay::primary("close"),
        }
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolSandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            self.browser
                .run(input, ctx.max_output_bytes)
                .await
                .map_err(exec_err)
        })
    }
}

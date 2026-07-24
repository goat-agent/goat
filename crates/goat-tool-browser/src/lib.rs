mod action;
mod dialog;
mod error;
mod navigation;
mod observe;
mod resilience;
mod session;
mod snapshot;
mod tool;

pub use error::BrowserError;
pub use tool::BrowserTool;

pub fn browser_tool() -> BrowserTool {
    BrowserTool::new(session::new_handle())
}

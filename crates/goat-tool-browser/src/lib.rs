mod action;
mod browser;
mod cdp;
mod dialog;
mod error;
mod keys;
mod navigation;
mod observe;
mod resilience;
mod session;
mod snapshot;
mod tool;
mod transport;

use std::sync::Arc;

pub use browser::{Browser, DESCRIPTION, NAME, parameters};
pub use error::BrowserError;
pub use tool::BrowserTool;
pub use transport::{Transport, TransportFuture};

pub fn browser_tool(transport: Arc<dyn Transport>) -> BrowserTool {
    BrowserTool::new(transport)
}

mod keys;
mod manager;
mod tool;

pub use manager::{MAX_SESSIONS, PtyManager};

use std::sync::Arc;

use goat_tool::ToolRegistry;

pub fn register(registry: &mut ToolRegistry, manager: Arc<PtyManager>) {
    registry.insert(Arc::new(tool::PtyTool { manager }));
}

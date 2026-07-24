mod keys;
mod manager;
mod tool;

pub use manager::{MAX_SESSIONS, PtyManager};

use std::sync::Arc;

use goat_agent_tool::ToolRegistry;

pub fn register(registry: &mut ToolRegistry, manager: Arc<PtyManager>) {
    registry.insert_handler(tool::spec(), Arc::new(tool::PtyTool { manager }), true);
}

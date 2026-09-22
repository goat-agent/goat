mod command_safety;
mod shell;

pub use command_safety::deny_reason;
pub use shell::NAME;

use std::sync::Arc;

use goat_tool::ToolRegistry;

pub fn register(registry: &mut ToolRegistry) {
    registry.insert(Arc::new(shell::ShellTool));
}

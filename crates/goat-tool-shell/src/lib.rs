pub mod agent;
mod background;
mod bash;

use std::sync::Arc;

pub use background::{
    BackgroundFuture, BackgroundProcessService, ProcessChunk, ProcessStart, all_with_background,
};
pub use bash::BashTool;
pub use bash::NAME as SHELL_TOOL;

pub fn all() -> Vec<Arc<dyn goat_tool::Tool>> {
    vec![Arc::new(BashTool)]
}

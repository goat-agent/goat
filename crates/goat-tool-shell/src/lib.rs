mod background;
mod bash;

pub use background::{
    BackgroundFuture, BackgroundProcessService, ProcessChunk, ProcessStart, all_with_background,
};
pub use bash::BashTool;

pub fn all() -> Vec<Box<dyn goat_tool::Tool>> {
    vec![Box::new(BashTool)]
}

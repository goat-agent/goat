pub mod agent;
mod error;
mod tools;

pub use tools::edit::EditTool;
pub use tools::read::ReadTool;
pub use tools::write::WriteTool;

pub fn all() -> Vec<std::sync::Arc<dyn goat_tool::Tool>> {
    vec![
        std::sync::Arc::new(ReadTool),
        std::sync::Arc::new(WriteTool),
        std::sync::Arc::new(EditTool),
    ]
}

mod tools;

pub use tools::edit::EditTool;
pub use tools::read::ReadTool;
pub use tools::write::WriteTool;

pub fn all() -> Vec<Box<dyn goat_tool::Tool>> {
    vec![Box::new(ReadTool), Box::new(WriteTool), Box::new(EditTool)]
}

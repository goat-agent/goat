mod bash;

pub use bash::BashTool;

pub fn all() -> Vec<Box<dyn goat_tool::Tool>> {
    vec![Box::new(BashTool)]
}

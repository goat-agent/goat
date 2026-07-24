mod glob;
mod grep;
mod web_search;

pub use glob::GlobTool;
pub use goat_search_provider::{
    SearchBuiltinTarget, SearchCredentialMetadata, SearchProviderKind, SearchProviderMetadata,
    SearchTargetMetadata,
};
pub use goat_search_providers::{
    build_search_account_config, configured_search_account, configured_search_provider,
    configured_search_target, default_search_target, is_builtin_search_target,
    search_builtin_targets, search_provider, search_providers,
};
pub use grep::GrepTool;
pub use web_search::WebSearchTool;

pub fn all() -> Vec<Box<dyn goat_tool::Tool>> {
    vec![
        Box::new(GrepTool),
        Box::new(GlobTool),
        Box::new(WebSearchTool::new()),
    ]
}

pub(crate) fn ignore_error(err: &ignore::Error) -> goat_tool::ToolError {
    goat_tool::ToolError::Io {
        path: "<glob/walk>".to_owned(),
        source: std::io::Error::other(err.to_string()),
    }
}

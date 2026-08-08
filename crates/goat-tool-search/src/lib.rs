mod glob;
mod grep;
mod native;
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
pub use native::{
    NativeSearchFuture, NativeSearchRequest, NativeSearchService, NativeWebSearchTool,
};
pub use web_search::WebSearchTool;

pub fn all_with_native(
    service: std::sync::Arc<dyn NativeSearchService>,
) -> Vec<Box<dyn goat_tool::Tool>> {
    let mut tools: Vec<Box<dyn goat_tool::Tool>> =
        vec![Box::new(NativeWebSearchTool::new(service))];
    tools.extend(all());
    tools
}

pub fn all() -> Vec<Box<dyn goat_tool::Tool>> {
    vec![
        Box::new(GrepTool),
        Box::new(GlobTool),
        Box::new(WebSearchTool::new()),
    ]
}

pub(crate) fn ignore_error(err: &ignore::Error) -> goat_tool::ToolError {
    goat_tool::ToolError::io(format!("io error on <glob/walk>: {err}"))
}

pub mod context;
pub mod display;
pub mod error;
pub mod gist;
pub mod name;
pub mod path;
pub mod policy;
pub mod registry;
pub mod selectors;
pub mod spec;
pub mod tool;

pub use context::ToolSandbox;
pub use error::{ToolError, ToolErrorClass};
pub use name::ToolName;
pub use policy::SandboxPolicy;
pub use registry::ToolRegistry;
pub use selectors::{selector_allows, validate_tool_selectors};
pub use spec::ToolSpec;
pub use tool::{
    AgentContext, Tool, ToolAudience, ToolBatchCall, ToolBatchFuture, ToolBatchInvocation,
    ToolCall, ToolContent, ToolContext, ToolDefinitionContext, ToolFuture, ToolHistoryGroup,
    ToolImage, ToolOutcomeExtension, ToolOutput, ToolReadSnapshot, ToolReadState, ToolSummaryKind,
};

pub const TRUNCATION_NOTICE: &str = "\n[output truncated]";

#[must_use]
pub fn truncate(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let boundary = text.floor_char_boundary(max_bytes);
    text.truncate(boundary);
    text.push_str(TRUNCATION_NOTICE);
    text
}

#[cfg(test)]
mod tests {
    use super::{TRUNCATION_NOTICE, ToolName, truncate};

    #[test]
    fn truncate_leaves_short_text_unchanged() {
        assert_eq!(truncate("hello".to_owned(), 100), "hello");
    }

    #[test]
    fn truncate_appends_notice_on_overflow() {
        let out = truncate("abcdefgh".to_owned(), 4);
        assert!(out.starts_with("abcd"));
        assert!(out.ends_with(TRUNCATION_NOTICE));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let out = truncate("héllo wörld".to_owned(), 2);
        assert!(out.starts_with('h'));
        assert!(out.ends_with(TRUNCATION_NOTICE));
    }

    #[test]
    fn validates_provider_safe_names() {
        assert!(ToolName::new("shell").is_ok());
        assert!(ToolName::new("DATA_EXPORT_v2").is_ok());
        assert!(ToolName::new("shell.run").is_err());
        assert!(ToolName::new("bad name").is_err());
    }
}

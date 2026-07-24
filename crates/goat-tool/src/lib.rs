pub mod context;
pub mod display;
pub mod error;
pub mod gist;
pub mod path;
pub mod policy;
pub mod spec;
pub mod tool;

pub use context::ToolContext;
pub use error::ToolError;
pub use policy::SandboxPolicy;
pub use spec::ToolSpec;
pub use tool::{Tool, ToolContent, ToolFuture, ToolImage, ToolOutput};

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
    use super::{TRUNCATION_NOTICE, truncate};

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
}

mod common;
mod edit;
mod glob;
mod grep;
mod read;
mod write;

pub use edit::NAME as EDIT_NAME;
pub use glob::NAME as GLOB_NAME;
pub use grep::NAME as GREP_NAME;
pub use read::NAME as READ_NAME;
pub use write::NAME as WRITE_NAME;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::{edit, glob, grep, read};
    use goat_tool::{ToolCall, ToolContext, ToolHandler, ToolName};
    use goat_types::{ChannelId, ConversationId, InstanceId, PersonaId};
    use serde_json::json;

    fn ctx(root: PathBuf) -> ToolContext {
        ToolContext {
            persona: PersonaId::from_slug("dev"),
            conversation: ConversationId::new(ChannelId::new("test"), InstanceId::new(), "x"),
            goat_root: root,
            read_state: Default::default(),
        }
    }

    #[tokio::test]
    async fn edit_requires_complete_read_and_replaces_unique_text() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
        let ctx = ctx(dir.path().to_path_buf());

        let denied = edit::EditTool
            .call(
                ctx.clone(),
                ToolCall {
                    call_id: "1".into(),
                    name: ToolName::from_static("edit"),
                    arguments: json!({"file_path":"a.txt","old_string":"world","new_string":"goat"}),
                },
            )
            .await;
        assert!(denied.is_error);

        let read = read::ReadTool
            .call(
                ctx.clone(),
                ToolCall {
                    call_id: "2".into(),
                    name: ToolName::from_static("read"),
                    arguments: json!({"file_path":"a.txt","limit":10}),
                },
            )
            .await;
        assert!(!read.is_error);

        let edited = edit::EditTool
            .call(
                ctx,
                ToolCall {
                    call_id: "3".into(),
                    name: ToolName::from_static("edit"),
                    arguments: json!({"file_path":"a.txt","old_string":"world","new_string":"goat"}),
                },
            )
            .await;
        assert!(!edited.is_error);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello\ngoat\n"
        );
    }

    #[tokio::test]
    async fn glob_and_grep_find_files_and_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn goat() {}\n").unwrap();
        fs::write(dir.path().join("README.md"), "goat\n").unwrap();
        let ctx = ctx(dir.path().to_path_buf());

        let globbed = glob::GlobTool
            .call(
                ctx.clone(),
                ToolCall {
                    call_id: "1".into(),
                    name: ToolName::from_static("glob"),
                    arguments: json!({"pattern":"**/*.rs"}),
                },
            )
            .await;
        assert!(!globbed.is_error);
        assert!(globbed.text_for_model().contains("lib.rs"));

        let grepped = grep::GrepTool
            .call(
                ctx,
                ToolCall {
                    call_id: "2".into(),
                    name: ToolName::from_static("grep"),
                    arguments: json!({"pattern":"goat","glob":"**/*.rs","literal":true}),
                },
            )
            .await;
        assert!(!grepped.is_error);
        assert!(grepped.text_for_model().contains("lib.rs"));
    }
}

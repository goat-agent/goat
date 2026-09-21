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

pub fn register(registry: &mut goat_tool::ToolRegistry) {
    registry.insert(std::sync::Arc::new(read::ReadTool));
    registry.insert(std::sync::Arc::new(write::WriteTool));
    registry.insert(std::sync::Arc::new(edit::EditTool));
    registry.insert(std::sync::Arc::new(glob::GlobTool));
    registry.insert(std::sync::Arc::new(grep::GrepTool));
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::agent::{edit, glob, grep, read};
    use goat_tool::{
        AgentContext, Tool, ToolCall, ToolContext, ToolName, ToolOutput, ToolReadState, ToolSandbox,
    };
    use goat_types::{AgentId, ChannelId, ConversationId, InstanceId};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn ctx(root: &Path) -> (ToolSandbox, CancellationToken, AgentContext) {
        let sandbox = ToolSandbox::rooted(root).unwrap();
        let token = CancellationToken::new();
        let agent = AgentContext {
            id: AgentId::from_slug("dev"),
            slug: "dev".into(),
            conversation: ConversationId::new(ChannelId::new("test"), InstanceId::new(), "x"),
            audience: None,
            read_state: ToolReadState::default(),
        };
        (sandbox, token, agent)
    }

    async fn read_path(ctx: ToolContext<'_>, path: &Path) -> ToolOutput {
        read::ReadTool
            .call(
                &ToolCall {
                    call_id: "path-policy".into(),
                    name: ToolName::from_static("read"),
                    arguments: json!({"file_path": path}),
                },
                ctx,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn path_inside_root_is_allowed() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("inside.txt"), "inside").unwrap();
        let (sandbox, token, agent) = ctx(root.path());
        let output = read_path(
            ToolContext::agent(&sandbox, &token, &agent),
            Path::new("inside.txt"),
        )
        .await;
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn absolute_path_outside_root_is_denied() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("outside.txt");
        fs::write(&path, "outside").unwrap();
        let (sandbox, token, agent) = ctx(root.path());
        let output = read_path(ToolContext::agent(&sandbox, &token, &agent), &path).await;
        assert!(output.is_error);
        assert!(output.text_for_model().contains("escapes agent tool root"));
    }

    #[tokio::test]
    async fn parent_traversal_is_denied() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(parent.path().join("outside.txt"), "outside").unwrap();
        let (sandbox, token, agent) = ctx(&root);
        let output = read_path(
            ToolContext::agent(&sandbox, &token, &agent),
            Path::new("../outside.txt"),
        )
        .await;
        assert!(output.is_error);
        assert!(output.text_for_model().contains("escapes agent tool root"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escape_is_denied() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside.txt"), "outside").unwrap();
        symlink(outside.path(), root.path().join("link")).unwrap();
        let (sandbox, token, agent) = ctx(root.path());
        let output = read_path(
            ToolContext::agent(&sandbox, &token, &agent),
            Path::new("link/outside.txt"),
        )
        .await;
        assert!(output.is_error);
        assert!(output.text_for_model().contains("escapes agent tool root"));
    }

    #[tokio::test]
    async fn edit_requires_complete_read_and_replaces_unique_text() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
        let (sandbox, token, agent) = ctx(dir.path());
        let ctx = ToolContext::agent(&sandbox, &token, &agent);

        let denied = edit::EditTool
            .call(
                &ToolCall {
                    call_id: "1".into(),
                    name: ToolName::from_static("edit"),
                    arguments: json!({"file_path":"a.txt","old_string":"world","new_string":"goat"}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(denied.is_error);

        let read = read::ReadTool
            .call(
                &ToolCall {
                    call_id: "2".into(),
                    name: ToolName::from_static("read"),
                    arguments: json!({"file_path":"a.txt","limit":10}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(!read.is_error);

        let edited = edit::EditTool
            .call(
                &ToolCall {
                    call_id: "3".into(),
                    name: ToolName::from_static("edit"),
                    arguments: json!({"file_path":"a.txt","old_string":"world","new_string":"goat"}),
                },
                ctx,
            )
            .await
            .unwrap();
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
        let (sandbox, token, agent) = ctx(dir.path());
        let ctx = ToolContext::agent(&sandbox, &token, &agent);

        let globbed = glob::GlobTool
            .call(
                &ToolCall {
                    call_id: "1".into(),
                    name: ToolName::from_static("glob"),
                    arguments: json!({"pattern":"**/*.rs"}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(!globbed.is_error);
        assert!(globbed.text_for_model().contains("lib.rs"));

        let grepped = grep::GrepTool
            .call(
                &ToolCall {
                    call_id: "2".into(),
                    name: ToolName::from_static("grep"),
                    arguments: json!({"pattern":"goat","glob":"**/*.rs","literal":true}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(!grepped.is_error);
        assert!(grepped.text_for_model().contains("lib.rs"));
    }
}

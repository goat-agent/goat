use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput, display};
use serde::Deserialize;

use crate::tools::relative_display;

pub struct WriteTool;

#[derive(Deserialize)]
struct Input {
    path: String,
    content: String,
}

impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Write content to a file in the session directory, creating parent directories and overwriting any existing file."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => ToolDisplay::primary(display::call_sig(
                "Write",
                &[display::flatten(&args.path).as_str()],
            )),
            Err(_) => display::generic(input),
        }
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let resolved = ctx.resolve(&args.path)?;
            ctx.ensure_writable(&resolved, &args.path)?;
            if let Some(parent) = resolved.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| ToolError::Io {
                        path: args.path.clone(),
                        source,
                    })?;
            }
            let line_count = args.content.lines().count();
            tokio::fs::write(&resolved, args.content.as_bytes())
                .await
                .map_err(|source| ToolError::Io {
                    path: args.path.clone(),
                    source,
                })?;
            let rel = relative_display(&ctx.cwd, &resolved);
            let unit = if line_count == 1 { "line" } else { "lines" };
            Ok(
                ToolOutput::text(format!("wrote {rel} ({line_count} {unit})"))
                    .with_summary(format!("{line_count} {unit}")),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::WriteTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let out = WriteTool
            .run(r#"{"path":"out.txt","content":"hello"}"#, &ctx)
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("wrote out.txt (1 line)"));
        assert_eq!(out.summary.as_deref(), Some("1 line"));
        let written = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
        assert_eq!(written, "hello");
    }

    #[tokio::test]
    async fn creates_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        WriteTool
            .run(r#"{"path":"a/b/c.txt","content":"x"}"#, &ctx)
            .await
            .unwrap();
        assert!(dir.path().join("a/b/c.txt").exists());
    }

    #[tokio::test]
    async fn escape_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let result = WriteTool
            .run(r#"{"path":"../evil.txt","content":"x"}"#, &ctx)
            .await;
        assert!(matches!(result, Err(ToolError::PathEscape { .. })));
    }

    #[tokio::test]
    async fn write_allow_restricts_to_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx(dir.path());
        let plan = ctx.cwd.join("PLAN.md");
        ctx.write_allow = Some(plan.clone());
        ctx.extra_path = Some(plan);
        let blocked = WriteTool
            .run(r#"{"path":"other.txt","content":"x"}"#, &ctx)
            .await;
        assert!(matches!(blocked, Err(ToolError::WriteBlocked { .. })));
        let allowed = WriteTool
            .run(r#"{"path":"PLAN.md","content":"plan"}"#, &ctx)
            .await;
        assert!(allowed.is_ok());
    }

    #[tokio::test]
    async fn extra_path_allows_file_outside_cwd() {
        let cwd = tempfile::tempdir().unwrap();
        let plandir = tempfile::tempdir().unwrap();
        let plan = plandir.path().canonicalize().unwrap().join("p.md");
        let mut ctx = ToolContext::new(cwd.path()).unwrap();
        ctx.extra_path = Some(plan.clone());
        ctx.write_allow = Some(plan.clone());
        let input =
            serde_json::json!({ "path": plan.to_str().unwrap(), "content": "x" }).to_string();
        let out = WriteTool.run(&input, &ctx).await;
        assert!(out.is_ok(), "plan file outside cwd must be writable");
        assert!(plan.exists());
    }
}

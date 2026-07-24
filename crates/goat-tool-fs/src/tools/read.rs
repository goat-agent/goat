use std::fmt::Write as _;

use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput, display};
use serde::Deserialize;

pub struct ReadTool;

#[derive(Deserialize)]
struct Input {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn description(&self) -> &'static str {
        "Read a file from the session directory, returning cat -n style numbered lines. Supports line-based offset and limit."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer"},
                "limit": {"type": "integer"}
            },
            "required": ["path"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => ToolDisplay::primary(display::call_sig(
                "Read",
                &[display::flatten(&args.path).as_str()],
            )),
            Err(_) => display::generic(input),
        }
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let resolved = ctx.resolve(&args.path)?;
            if !resolved.exists() {
                return Err(ToolError::NotFound { path: args.path });
            }
            let file = tokio::fs::File::open(&resolved)
                .await
                .map_err(|source| ToolError::Io {
                    path: args.path.clone(),
                    source,
                })?;
            let mut reader = tokio::io::BufReader::new(file);

            let start = args.offset.unwrap_or(1).max(1);
            let max_bytes = ctx.max_output_bytes;
            let mut out = String::new();
            let mut lineno = 0usize;
            let mut emitted = 0usize;
            let mut truncated = false;
            let mut buf = Vec::new();
            loop {
                if let Some(limit) = args.limit
                    && emitted >= limit
                {
                    break;
                }
                buf.clear();
                let read = tokio::io::AsyncBufReadExt::read_until(&mut reader, b'\n', &mut buf)
                    .await
                    .map_err(|source| ToolError::Io {
                        path: args.path.clone(),
                        source,
                    })?;
                if read == 0 {
                    break;
                }
                lineno += 1;
                if lineno < start {
                    continue;
                }
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\n', '\r']);
                let _ = writeln!(out, "{lineno:>6}\t{line}");
                emitted += 1;
                if out.len() > max_bytes {
                    truncated = true;
                    break;
                }
            }

            if out.len() > max_bytes {
                let boundary = out.floor_char_boundary(max_bytes);
                out.truncate(boundary);
                truncated = true;
            }
            if truncated {
                out.push_str("\n[output truncated]\n");
            }
            Ok(ToolOutput::text(out))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::ReadTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[test]
    fn display_shows_call_signature() {
        let display = ReadTool.display_input(r#"{"path":"a.txt","offset":120,"limit":50}"#);
        assert_eq!(display.primary, "Read(a.txt)");
        assert_eq!(display.detail, None);
    }

    #[tokio::test]
    async fn read_has_no_summary() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool.run(r#"{"path":"a.txt"}"#, &ctx).await.unwrap();
        assert_eq!(out.summary, None);
    }

    #[tokio::test]
    async fn reads_numbered_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool.run(r#"{"path":"a.txt"}"#, &ctx).await.unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("     1\tone"));
        assert!(text.contains("     2\ttwo"));
        assert!(text.contains("     3\tthree"));
    }

    #[tokio::test]
    async fn applies_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "1\n2\n3\n4\n5\n").unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool
            .run(r#"{"path":"a.txt","offset":2,"limit":2}"#, &ctx)
            .await
            .unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("     2\t2"));
        assert!(text.contains("     3\t3"));
        assert!(!text.contains("     1\t1"));
        assert!(!text.contains("     4\t4"));
    }

    #[tokio::test]
    async fn caps_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(300 * 1024);
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool.run(r#"{"path":"big.txt"}"#, &ctx).await.unwrap();
        assert!(out.as_text().unwrap().contains("[output truncated]"));
    }

    #[tokio::test]
    async fn missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let result = ReadTool.run(r#"{"path":"nope.txt"}"#, &ctx).await;
        assert!(matches!(result, Err(ToolError::NotFound { .. })));
    }

    #[tokio::test]
    async fn large_file_offset_works() {
        let dir = tempfile::tempdir().unwrap();
        let content: String = (1..=200).fold(String::new(), |mut s, i| {
            let _ = writeln!(s, "line {i}");
            s
        });
        std::fs::write(dir.path().join("big.txt"), &content).unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool
            .run(r#"{"path":"big.txt","offset":100,"limit":5}"#, &ctx)
            .await
            .unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("   100\tline 100"));
        assert!(text.contains("   104\tline 104"));
        assert!(!text.contains("     1\tline 1"));
        assert!(!text.contains("   105\tline 105"));
    }
}

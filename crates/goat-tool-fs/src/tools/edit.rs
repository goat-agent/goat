use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput, display};
use serde::Deserialize;

use crate::tools::relative_display;

pub struct EditTool;

#[derive(Deserialize)]
struct Input {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Replace occurrences of old_string with new_string in a file. By default requires a single unique match; set replace_all to replace every occurrence."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean"}
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        match serde_json::from_str::<Input>(input) {
            Ok(args) => ToolDisplay::primary(display::call_sig(
                "Edit",
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
            if !resolved.exists() {
                return Err(ToolError::NotFound { path: args.path });
            }
            let bytes = tokio::fs::read(&resolved)
                .await
                .map_err(|source| ToolError::Io {
                    path: args.path.clone(),
                    source,
                })?;
            let contents = String::from_utf8_lossy(&bytes).into_owned();
            let count = contents.matches(&args.old_string).count();
            if count == 0 {
                return Err(ToolError::EditNoMatch { path: args.path });
            }
            if !args.replace_all && count > 1 {
                return Err(ToolError::EditNotUnique { path: args.path });
            }
            let replaced = if args.replace_all { count } else { 1 };
            let updated = if args.replace_all {
                contents.replace(&args.old_string, &args.new_string)
            } else {
                contents.replacen(&args.old_string, &args.new_string, 1)
            };
            tokio::fs::write(&resolved, updated.as_bytes())
                .await
                .map_err(|source| ToolError::Io {
                    path: args.path.clone(),
                    source,
                })?;
            let rel = relative_display(&ctx.cwd, &resolved);
            let removed = args.old_string.lines().count() * replaced;
            let added = args.new_string.lines().count() * replaced;
            let summary = diff_summary(&args.old_string, &args.new_string, replaced);
            let output = ToolOutput::text(format!("edited {rel} (+{added} -{removed})"));
            Ok(match summary {
                Some(diff) => output.with_summary(diff),
                None => output,
            })
        })
    }
}

const DIFF_SIDE_CAP: usize = 4;

fn push_side(out: &mut Vec<String>, lines: &[&str], sign: char) {
    if lines.len() <= DIFF_SIDE_CAP {
        for line in lines {
            out.push(format!("{sign} {line}"));
        }
        return;
    }
    for line in &lines[..DIFF_SIDE_CAP - 1] {
        out.push(format!("{sign} {line}"));
    }
    out.push(format!(
        "{sign} … {} more lines",
        lines.len() - (DIFF_SIDE_CAP - 1)
    ));
}

fn diff_summary(old: &str, new: &str, replaced: usize) -> Option<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut start = 0;
    while start < old_lines.len() && start < new_lines.len() && old_lines[start] == new_lines[start]
    {
        start += 1;
    }
    let mut old_end = old_lines.len();
    let mut new_end = new_lines.len();
    while old_end > start && new_end > start && old_lines[old_end - 1] == new_lines[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    let mut out: Vec<String> = Vec::new();
    if replaced > 1 {
        out.push(format!("{replaced} replacements"));
    }
    push_side(&mut out, &old_lines[start..old_end], '-');
    push_side(&mut out, &new_lines[start..new_end], '+');
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::EditTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn replaces_single() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        let ctx = ctx(dir.path());
        let out = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"world","new_string":"there"}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("edited a.txt (+1 -1)"));
        assert_eq!(out.summary.as_deref(), Some("- world\n+ there"));
        let result = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(result, "hello there");
    }

    #[tokio::test]
    async fn no_match_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let ctx = ctx(dir.path());
        let result = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"absent","new_string":"x"}"#,
                &ctx,
            )
            .await;
        assert!(matches!(result, Err(ToolError::EditNoMatch { .. })));
    }

    #[tokio::test]
    async fn not_unique_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a a a").unwrap();
        let ctx = ctx(dir.path());
        let result = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"a","new_string":"b"}"#,
                &ctx,
            )
            .await;
        assert!(matches!(result, Err(ToolError::EditNotUnique { .. })));
    }

    #[tokio::test]
    async fn replace_all_replaces_every() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a a a").unwrap();
        let ctx = ctx(dir.path());
        let out = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"a","new_string":"b","replace_all":true}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("edited a.txt (+3 -3)"));
        assert_eq!(out.summary.as_deref(), Some("3 replacements\n- a\n+ b"));
        let result = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(result, "b b b");
    }

    #[test]
    fn diff_trims_common_context_lines() {
        let old = "fn foo() {\n    let x = 1;\n}";
        let new = "fn foo() {\n    let x = 2;\n}";
        assert_eq!(
            super::diff_summary(old, new, 1).as_deref(),
            Some("-     let x = 1;\n+     let x = 2;")
        );
    }

    #[test]
    fn diff_caps_long_sides() {
        let old = "a\nb\nc\nd\ne\nf";
        let new = "z";
        let diff = super::diff_summary(old, new, 1).unwrap();
        assert!(diff.contains("- … 3 more lines"));
        assert!(diff.contains("+ z"));
    }

    #[test]
    fn pure_insertion_emits_only_additions() {
        let diff = super::diff_summary("anchor", "anchor\nadded", 1).unwrap();
        assert_eq!(diff, "+ added");
    }
}

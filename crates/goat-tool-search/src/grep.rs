use std::fmt::Write as _;
use std::io::BufRead as _;

use goat_tool::{
    Tool, ToolContext, ToolError, ToolFuture, ToolOutput,
    path::{blocked_path, relative_display},
};
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use serde::Deserialize;

use crate::ignore_error;

const DEFAULT_MAX_RESULTS: usize = 100;
const MAX_RESULTS_CAP: usize = 10_000;
const REGEX_SIZE_LIMIT: usize = 1 << 20;

pub struct GrepTool;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    max_results: Option<usize>,
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Search file contents for a regular expression across the session directory, honoring .gitignore. Optionally restrict to a subpath or glob."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string"},
                "glob": {"type": "string"},
                "max_results": {"type": "integer"}
            },
            "required": ["pattern"]
        })
    }

    fn display_input(&self, input: &str) -> goat_protocol::ToolDisplay {
        let Ok(args) = serde_json::from_str::<Input>(input) else {
            return goat_tool::display::generic(input);
        };
        let pattern = goat_tool::display::flatten(&args.pattern);
        let mut params = vec![pattern];
        if let Some(path) = args.path.filter(|p| !p.is_empty() && p != ".") {
            params.push(path);
        }
        let refs: Vec<&str> = params.iter().map(String::as_str).collect();
        goat_protocol::ToolDisplay::primary(goat_tool::display::call_sig("Grep", &refs))
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let root = match &args.path {
                Some(path) => ctx.resolve(path)?,
                None => ctx.cwd.clone(),
            };
            let cwd = ctx.cwd.clone();
            let blocked = ctx.blocked_paths.clone();
            let max_results = args
                .max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .min(MAX_RESULTS_CAP);
            let max_output_bytes = ctx.max_output_bytes;

            let join = tokio::task::spawn_blocking(move || {
                search(
                    &cwd,
                    &root,
                    &blocked,
                    &args.pattern,
                    args.glob.as_deref(),
                    max_results,
                    max_output_bytes,
                )
            })
            .await;

            match join {
                Ok(result) => result.map(ToolOutput::text),
                Err(err) => Ok(ToolOutput::text(format!("search task failed: {err}"))),
            }
        })
    }
}

fn search(
    cwd: &std::path::Path,
    root: &std::path::Path,
    blocked: &[std::path::PathBuf],
    pattern: &str,
    glob: Option<&str>,
    max_results: usize,
    max_output_bytes: usize,
) -> Result<String, ToolError> {
    let regex = regex::RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()?;
    let mut builder = WalkBuilder::new(root);
    builder.require_git(false);
    builder.hidden(false);
    let blocked_for_walk = blocked.to_vec();
    builder.filter_entry(move |entry| !blocked_path(&blocked_for_walk, entry.path()));
    let matcher = match glob {
        Some(glob) => {
            let mut overrides = OverrideBuilder::new(root);
            overrides.add(glob).map_err(|err| ignore_error(&err))?;
            Some(overrides.build().map_err(|err| ignore_error(&err))?)
        }
        None => None,
    };

    let mut out = String::new();
    let mut count = 0usize;
    let mut truncated = false;

    'walk: for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        if let Some(matcher) = &matcher
            && !matcher.matched(entry.path(), false).is_whitelist()
        {
            continue;
        }
        let display = relative_display(cwd, entry.path());
        let Ok(file_hits) = matching_lines(entry.path(), &display, &regex, max_results - count)
        else {
            continue;
        };
        let overflowed = file_hits.overflowed;
        for line in file_hits.lines {
            if count >= max_results {
                truncated = true;
                break 'walk;
            }
            out.push_str(&line);
            count += 1;
            if out.len() >= max_output_bytes {
                truncated = true;
                break 'walk;
            }
        }
        if overflowed {
            truncated = true;
            break 'walk;
        }
    }

    if count == 0 {
        return Ok("no matches".to_owned());
    }
    if truncated {
        out.push_str("\n[output truncated]");
    }
    Ok(out)
}

struct FileHits {
    lines: Vec<String>,
    overflowed: bool,
}

fn matching_lines(
    path: &std::path::Path,
    display: &str,
    regex: &regex::Regex,
    max_matches: usize,
) -> std::io::Result<FileHits> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut lines = Vec::new();
    let mut overflowed = false;
    let mut line = String::new();
    let mut lineno = 0usize;
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        lineno += 1;
        let normalized = normalize_line(&line);
        if regex.is_match(normalized) {
            if lines.len() >= max_matches {
                overflowed = true;
                continue;
            }
            let line_display = if normalized.len() > 1024 {
                let b = normalized.floor_char_boundary(1024);
                format!("{}\u{2026}", &normalized[..b])
            } else {
                normalized.to_owned()
            };
            let mut rendered = String::new();
            let _ = writeln!(rendered, "{display}:{lineno}: {line_display}");
            lines.push(rendered);
        }
    }
    Ok(FileHits { lines, overflowed })
}

fn normalize_line(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::GrepTool;
    use goat_tool::{Tool, ToolContext};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn finds_matching_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta needle\ngamma\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"needle"}"#, &ctx).await.unwrap();
        assert!(out.as_text().unwrap().contains("a.txt:2: beta needle"));
    }

    #[tokio::test]
    async fn no_match_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "nothing here\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"absent"}"#, &ctx).await.unwrap();
        assert_eq!(out.as_text().unwrap(), "no matches");
    }

    #[tokio::test]
    async fn respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "skipped/\n").unwrap();
        std::fs::create_dir(dir.path().join("skipped")).unwrap();
        std::fs::write(dir.path().join("skipped/a.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("kept.txt"), "needle\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"needle"}"#, &ctx).await.unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("kept.txt"));
        assert!(!text.contains("skipped"));
    }

    #[tokio::test]
    async fn managed_worktrees_are_hidden_from_default_search() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat/worktrees/plan")).unwrap();
        std::fs::write(
            dir.path().join(".goat/worktrees/plan/hidden.txt"),
            "needle\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("visible.txt"), "needle\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"needle"}"#, &ctx).await.unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("visible.txt"));
        assert!(!text.contains("hidden.txt"));
    }

    #[tokio::test]
    async fn explicit_managed_worktree_path_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat/worktrees/plan")).unwrap();
        let ctx = ctx(dir.path());
        let result = GrepTool
            .run(r#"{"pattern":"needle","path":".goat/worktrees"}"#, &ctx)
            .await;
        assert!(matches!(
            result,
            Err(goat_tool::ToolError::PathBlocked { .. })
        ));
    }

    #[tokio::test]
    async fn reports_truncation_when_same_file_has_more_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.txt"),
            "needle 1
needle 2
",
        )
        .unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool
            .run(r#"{"pattern":"needle","max_results":1}"#, &ctx)
            .await
            .unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("a.txt:1: needle 1"));
        assert!(text.contains("[output truncated]"));
    }

    #[tokio::test]
    async fn skips_non_utf8_file_without_consuming_limits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.txt"), b"needle\n\xFF\n").unwrap();
        std::fs::write(dir.path().join("good.txt"), "needle\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool
            .run(r#"{"pattern":"needle","max_results":1}"#, &ctx)
            .await
            .unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("good.txt:1: needle"));
        assert!(!text.contains("bad.txt"));
    }

    #[tokio::test]
    async fn truncates_long_utf8_line_on_char_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let line = format!("needle {}", "é".repeat(600));
        std::fs::write(dir.path().join("a.txt"), format!("{line}\n")).unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"needle"}"#, &ctx).await.unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains('\u{2026}'));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }

    #[tokio::test]
    async fn crlf_matches_without_carriage_return() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\r\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool
            .run(r#"{"pattern":"needle$"}"#, &ctx)
            .await
            .unwrap();
        assert_eq!(out.as_text().unwrap(), "a.txt:1: needle\n");
    }
}

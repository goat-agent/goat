use std::fmt::Write as _;

use goat_tool::{
    Tool, ToolContext, ToolError, ToolFuture, ToolOutput,
    path::{blocked_path, relative_display},
};
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use serde::Deserialize;

use crate::ignore_error;

pub struct GlobTool;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    path: Option<String>,
}

impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "List files in the session directory matching a glob pattern, honoring .gitignore. Results are relative paths, sorted."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string"}
            },
            "required": ["pattern"]
        })
    }

    fn display_input(&self, input: &str) -> goat_protocol::ToolDisplay {
        let Ok(args) = serde_json::from_str::<Input>(input) else {
            return goat_tool::display::generic(input);
        };
        let pattern = goat_tool::display::flatten(&args.pattern);
        let sig = match args.path.filter(|p| !p.is_empty() && p != ".") {
            Some(path) => goat_tool::display::call_sig("Glob", &[pattern.as_str(), path.as_str()]),
            None => goat_tool::display::call_sig("Glob", &[pattern.as_str()]),
        };
        goat_protocol::ToolDisplay::primary(sig)
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

            let join =
                tokio::task::spawn_blocking(move || walk(&cwd, &root, &blocked, &args.pattern))
                    .await;

            match join {
                Ok(result) => result.map(ToolOutput::text),
                Err(err) => Ok(ToolOutput::text(format!("glob task failed: {err}"))),
            }
        })
    }
}

const MAX_GLOB_RESULTS: usize = 1000;

fn walk(
    cwd: &std::path::Path,
    root: &std::path::Path,
    blocked: &[std::path::PathBuf],
    pattern: &str,
) -> Result<String, ToolError> {
    let mut overrides = OverrideBuilder::new(root);
    overrides.add(pattern).map_err(|err| ignore_error(&err))?;
    let matcher = overrides.build().map_err(|err| ignore_error(&err))?;

    let mut found = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder.require_git(false);
    builder.hidden(false);
    let blocked_for_walk = blocked.to_vec();
    builder.filter_entry(move |entry| !blocked_path(&blocked_for_walk, entry.path()));
    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        if !matcher.matched(entry.path(), false).is_whitelist() {
            continue;
        }
        found.push(relative_display(cwd, entry.path()));
    }

    if found.is_empty() {
        return Ok("no files".to_owned());
    }
    found.sort();
    let total = found.len();
    found.truncate(MAX_GLOB_RESULTS);
    let mut out = found.join("\n");
    if total > MAX_GLOB_RESULTS {
        let _ = write!(
            out,
            "\n[{} more files truncated]\n",
            total - MAX_GLOB_RESULTS
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::GlobTool;
    use goat_tool::{Tool, ToolContext};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn matches_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();
        let ctx = ctx(dir.path());
        let out = GlobTool.run(r#"{"pattern":"*.rs"}"#, &ctx).await.unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("a.rs"));
        assert!(text.contains("b.rs"));
        assert!(!text.contains("c.txt"));
        assert_eq!(out.summary, None);
    }

    #[tokio::test]
    async fn no_files_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let ctx = ctx(dir.path());
        let out = GlobTool.run(r#"{"pattern":"*.rs"}"#, &ctx).await.unwrap();
        assert_eq!(out.as_text().unwrap(), "no files");
        assert_eq!(out.summary, None);
    }

    #[test]
    fn display_omits_trivial_scope() {
        use goat_tool::Tool;
        let display = GlobTool.display_input(r#"{"pattern":"*.rs","path":"."}"#);
        assert_eq!(display.primary, "Glob(*.rs)");
        assert_eq!(display.detail, None);
    }

    #[tokio::test]
    async fn respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/built.rs"), "").unwrap();
        std::fs::write(dir.path().join("kept.rs"), "").unwrap();
        let ctx = ctx(dir.path());
        let out = GlobTool.run(r#"{"pattern":"*.rs"}"#, &ctx).await.unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("kept.rs"));
        assert!(!text.contains("built.rs"));
    }

    #[tokio::test]
    async fn managed_worktrees_are_hidden_from_default_search() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat/worktrees/plan")).unwrap();
        std::fs::write(dir.path().join(".goat/worktrees/plan/hidden.rs"), "").unwrap();
        std::fs::write(dir.path().join("visible.rs"), "").unwrap();
        let ctx = ctx(dir.path());
        let out = GlobTool.run(r#"{"pattern":"*.rs"}"#, &ctx).await.unwrap();
        let text = out.as_text().unwrap();
        assert!(text.contains("visible.rs"));
        assert!(!text.contains("hidden.rs"));
    }

    #[tokio::test]
    async fn explicit_managed_worktree_path_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".goat/worktrees/plan")).unwrap();
        let ctx = ctx(dir.path());
        let result = GlobTool
            .run(r#"{"pattern":"*.rs","path":".goat/worktrees"}"#, &ctx)
            .await;
        assert!(matches!(
            result,
            Err(goat_tool::ToolError::PathBlocked { .. })
        ));
    }

    #[tokio::test]
    async fn lists_dot_github_in_real_repo_layout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        std::fs::write(dir.path().join(".github/workflows/ci.yml"), "").unwrap();
        let ctx = ctx(dir.path());
        for pat in ["**/.github/**", ".github/**/*", ".github/workflows/*.yml"] {
            let out = GlobTool
                .run(&format!(r#"{{"pattern":"{pat}"}}"#), &ctx)
                .await
                .unwrap_or_else(|e| panic!("{pat}: {e:?}"));
            let text = out.as_text().unwrap();
            assert!(
                text.contains("ci.yml") || text.contains(".github"),
                "{pat} got: {text}"
            );
        }
    }

    #[test]
    fn github_style_patterns_parse() {
        use ignore::overrides::OverrideBuilder;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let root = root.canonicalize().unwrap();
        for pat in ["**/.github/**", ".github/**/*", "**/*depend*"] {
            let mut o = OverrideBuilder::new(&root);
            o.add(pat).unwrap_or_else(|e| panic!("add {pat}: {e:?}"));
            o.build().unwrap_or_else(|e| panic!("build {pat}: {e:?}"));
        }
    }
}

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use globset::Glob;
use goat_tool::{ToolCall, ToolContext, ToolFactory, ToolHandler, ToolName, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::json;
use walkdir::{DirEntry, WalkDir};

use crate::common::{self, DEFAULT_LIST_LIMIT};

pub const NAME: ToolName = ToolName::from_static("glob");

pub struct GlobTool;

#[derive(Debug, Deserialize)]
struct GlobArgs {
    pattern: String,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    include_hidden: bool,
}

#[async_trait]
impl ToolHandler for GlobTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args = match serde_json::from_value::<GlobArgs>(call.arguments) {
            Ok(args) => args,
            Err(e) => return common::error(format!("invalid glob input: {e}")),
        };
        if args.pattern.trim().is_empty() {
            return common::error("pattern must not be empty");
        }
        let root = match args.path.as_deref() {
            Some(path) => match common::existing_path(&ctx.goat_root, path) {
                Ok(path) => path,
                Err(e) => return common::error(e),
            },
            None => ctx.goat_root.clone(),
        };
        let matcher = match Glob::new(&args.pattern).map(|g| g.compile_matcher()) {
            Ok(matcher) => matcher,
            Err(e) => return common::error(format!("invalid glob pattern: {e}")),
        };
        let limit = common::limit(args.limit, DEFAULT_LIST_LIMIT);
        let mut matches = Vec::new();
        for entry in WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| args.include_hidden || !is_hidden(entry, &root))
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let candidate = if Path::new(&args.pattern).is_absolute() {
                entry.path()
            } else {
                entry.path().strip_prefix(&root).unwrap_or(entry.path())
            };
            if matcher.is_match(candidate) {
                let modified_ms = entry.metadata().ok().and_then(|m| common::modified_ms(&m));
                matches.push((entry.path().to_path_buf(), modified_ms));
            }
        }
        matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let truncated = matches.len() > limit;
        let paths: Vec<String> = matches
            .into_iter()
            .take(limit)
            .map(|(path, _)| path.display().to_string())
            .collect();
        ToolOutput::structured(json!({
            "matches": paths,
            "truncated": truncated,
        }))
    }
}

fn is_hidden(entry: &DirEntry, root: &Path) -> bool {
    entry
        .path()
        .strip_prefix(root)
        .ok()
        .is_some_and(common::is_hidden_component)
}

fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        NAME,
        "Find files by glob pattern. Use for file discovery instead of shell ls/find when only paths are needed.",
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern, e.g. **/*.rs." },
                "path": { "type": "string", "description": "Optional root directory. Relative paths resolve under the goat root." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000 },
                "include_hidden": { "type": "boolean", "description": "Include dotfiles and hidden directories." }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
    );
    spec.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "matches": { "type": "array", "items": { "type": "string" } },
            "truncated": { "type": "boolean" }
        },
        "required": ["matches", "truncated"],
        "additionalProperties": false
    }));
    spec
}

fn ctor() -> Arc<dyn ToolHandler> {
    Arc::new(GlobTool)
}

inventory::submit! {
    ToolFactory { name: NAME, default_enabled: true, spec, ctor }
}

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use globset::Glob;
use goat_tool::{
    Tool, ToolCall, ToolContext, ToolDefinitionContext, ToolFuture, ToolName, ToolOutput, ToolSpec,
};
use serde::Deserialize;
use serde_json::json;
use walkdir::{DirEntry, WalkDir};

use crate::agent::common::{self, DEFAULT_LIST_LIMIT};

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

impl Tool for GlobTool {
    fn name(&self) -> ToolName {
        NAME.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Find files by glob pattern. Use for file discovery instead of shell ls/find when only paths are needed.".into()
    }

    fn parameters(&self) -> serde_json::Value {
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
        })
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let args = match serde_json::from_value::<GlobArgs>(call.arguments.clone()) {
                Ok(args) => args,
                Err(e) => return Ok(common::error(format!("invalid glob input: {e}"))),
            };
            if args.pattern.trim().is_empty() {
                return Ok(common::error("pattern must not be empty"));
            }
            let root = match args.path.as_deref() {
                Some(path) => match common::existing_path(&ctx, path) {
                    Ok(path) => path,
                    Err(e) => return Ok(common::error(e)),
                },
                None => match common::existing_path(&ctx, Path::new(".")) {
                    Ok(path) => path,
                    Err(e) => return Ok(common::error(e)),
                },
            };
            let glob_matcher = match Glob::new(&args.pattern).map(|g| g.compile_matcher()) {
                Ok(m) => m,
                Err(e) => return Ok(common::error(format!("invalid glob pattern: {e}"))),
            };
            let limit = common::limit(args.limit, DEFAULT_LIST_LIMIT);
            let mut matches = Vec::new();
            for entry in WalkDir::new(&root)
                .into_iter()
                .filter_entry(|entry| args.include_hidden || !is_hidden(entry, &root))
            {
                let Ok(entry) = entry else {
                    continue;
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let candidate = if Path::new(&args.pattern).is_absolute() {
                    entry.path()
                } else {
                    entry.path().strip_prefix(&root).unwrap_or(entry.path())
                };
                if glob_matcher.is_match(candidate) {
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
            Ok(ToolOutput::structured(json!({
                "matches": paths,
                "truncated": truncated,
            })))
        })
    }

    fn definition(&self, _ctx: ToolDefinitionContext) -> Option<ToolSpec> {
        Some(spec())
    }
}

fn is_hidden(entry: &DirEntry, root: &Path) -> bool {
    entry
        .path()
        .strip_prefix(root)
        .is_ok_and(common::is_hidden_component)
}

fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        NAME.clone(),
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

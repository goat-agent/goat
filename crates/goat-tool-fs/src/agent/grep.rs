use std::borrow::Cow;
use std::path::{Path, PathBuf};

use globset::Glob;
use goat_tool::{
    Tool, ToolCall, ToolContext, ToolDefinitionContext, ToolFuture, ToolName, ToolOutput, ToolSpec,
};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::json;
use walkdir::{DirEntry, WalkDir};

use crate::agent::common::{self, MAX_MATCHES};

pub const NAME: ToolName = ToolName::from_static("grep");

pub struct GrepTool;

#[derive(Debug, Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    literal: bool,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    include_hidden: bool,
}

impl Tool for GrepTool {
    fn name(&self) -> ToolName {
        NAME.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Search UTF-8 text files by regex or literal text. Use for content discovery instead of shell grep/rg when structured matches are enough.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern by default, or literal text when literal=true." },
                "path": { "type": "string", "description": "Optional root directory. Relative paths resolve under the goat root." },
                "glob": { "type": "string", "description": "Optional file glob filter, e.g. **/*.rs." },
                "literal": { "type": "boolean" },
                "case_insensitive": { "type": "boolean" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000 },
                "include_hidden": { "type": "boolean" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let args = match serde_json::from_value::<GrepArgs>(call.arguments.clone()) {
                Ok(args) => args,
                Err(e) => return Ok(common::error(format!("invalid grep input: {e}"))),
            };
            if args.pattern.is_empty() {
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
            let pattern = if args.literal {
                regex::escape(&args.pattern)
            } else {
                args.pattern.clone()
            };
            let re = match RegexBuilder::new(&pattern)
                .case_insensitive(args.case_insensitive)
                .build()
            {
                Ok(re) => re,
                Err(e) => return Ok(common::error(format!("invalid grep regex: {e}"))),
            };
            let glob = match args.glob.as_deref() {
                Some(pattern) => match Glob::new(pattern).map(|g| g.compile_matcher()) {
                    Ok(glob) => Some(glob),
                    Err(e) => return Ok(common::error(format!("invalid file glob: {e}"))),
                },
                None => None,
            };
            let limit = args.limit.unwrap_or(MAX_MATCHES).clamp(1, MAX_MATCHES);
            let mut matches = Vec::new();
            'files: for entry in WalkDir::new(&root)
                .into_iter()
                .filter_entry(|entry| args.include_hidden || !is_hidden(entry, &root))
            {
                let Ok(entry) = entry else {
                    continue;
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                if glob.as_ref().is_some_and(|g| !g.is_match(rel)) {
                    continue;
                }
                let Ok(content) = common::read_text(entry.path()) else {
                    continue;
                };
                for (line_idx, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        matches.push(json!({
                            "file_path": entry.path(),
                            "line_number": line_idx + 1,
                            "line": line,
                        }));
                        if matches.len() >= limit {
                            break 'files;
                        }
                    }
                }
            }
            let truncated = matches.len() >= limit;
            Ok(ToolOutput::structured(json!({
                "matches": matches,
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
        "Search UTF-8 text files by regex or literal text. Use for content discovery instead of shell grep/rg when structured matches are enough.",
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern by default, or literal text when literal=true." },
                "path": { "type": "string", "description": "Optional root directory. Relative paths resolve under the goat root." },
                "glob": { "type": "string", "description": "Optional file glob filter, e.g. **/*.rs." },
                "literal": { "type": "boolean" },
                "case_insensitive": { "type": "boolean" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000 },
                "include_hidden": { "type": "boolean" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
    );
    spec.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "matches": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "line_number": { "type": "integer" },
                        "line": { "type": "string" }
                    },
                    "required": ["file_path", "line_number", "line"],
                    "additionalProperties": false
                }
            },
            "truncated": { "type": "boolean" }
        },
        "required": ["matches", "truncated"],
        "additionalProperties": false
    }));
    spec
}

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use globset::Glob;
use goat_tool::{ToolCall, ToolContext, ToolFactory, ToolHandler, ToolName, ToolOutput, ToolSpec};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::json;
use walkdir::{DirEntry, WalkDir};

use crate::common::{self, MAX_MATCHES};

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

#[async_trait]
impl ToolHandler for GrepTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args = match serde_json::from_value::<GrepArgs>(call.arguments) {
            Ok(args) => args,
            Err(e) => return common::error(format!("invalid grep input: {e}")),
        };
        if args.pattern.is_empty() {
            return common::error("pattern must not be empty");
        }
        let root = match args.path.as_deref() {
            Some(path) => match common::existing_path(&ctx.goat_root, path) {
                Ok(path) => path,
                Err(e) => return common::error(e),
            },
            None => ctx.goat_root.clone(),
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
            Err(e) => return common::error(format!("invalid grep regex: {e}")),
        };
        let glob = match args.glob.as_deref() {
            Some(pattern) => match Glob::new(pattern).map(|g| g.compile_matcher()) {
                Ok(glob) => Some(glob),
                Err(e) => return common::error(format!("invalid file glob: {e}")),
            },
            None => None,
        };
        let limit = args.limit.unwrap_or(MAX_MATCHES).clamp(1, MAX_MATCHES);
        let mut matches = Vec::new();
        'files: for entry in WalkDir::new(&root)
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
            let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            if glob.as_ref().is_some_and(|g| !g.is_match(rel)) {
                continue;
            }
            let content = match common::read_text(entry.path()) {
                Ok(content) => content,
                Err(_) => continue,
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
        ToolOutput::structured(json!({
            "matches": matches,
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

fn ctor() -> Arc<dyn ToolHandler> {
    Arc::new(GrepTool)
}

inventory::submit! {
    ToolFactory { name: NAME, default_enabled: true, spec, ctor }
}

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use goat_tool::{ToolCall, ToolContext, ToolFactory, ToolHandler, ToolName, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::json;

use crate::common::{self, DEFAULT_READ_LINES};

pub const NAME: ToolName = ToolName::from_static("read");

pub struct ReadTool;

#[derive(Debug, Deserialize)]
struct ReadArgs {
    file_path: PathBuf,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for ReadTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args = match serde_json::from_value::<ReadArgs>(call.arguments) {
            Ok(args) => args,
            Err(e) => return common::error(format!("invalid read input: {e}")),
        };
        let path = match common::existing_path(&ctx.goat_root, &args.file_path) {
            Ok(path) => path,
            Err(e) => return common::error(e),
        };
        let content = match common::read_text(&path) {
            Ok(content) => content,
            Err(e) => return common::error(e),
        };
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let offset = args.offset.unwrap_or(1).max(1);
        let limit = args.limit.unwrap_or(DEFAULT_READ_LINES).max(1);
        let start_idx = offset.saturating_sub(1).min(total_lines);
        let end_idx = (start_idx + limit).min(total_lines);
        let complete = start_idx == 0 && end_idx == total_lines;

        let snap = match common::snapshot(&path, &content, complete) {
            Ok(snap) => snap,
            Err(e) => return common::error(e),
        };
        if let Err(e) = common::store_snapshot(&ctx, path.clone(), snap) {
            return common::error(e);
        }

        let numbered = lines[start_idx..end_idx]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", start_idx + i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");
        ToolOutput::structured(json!({
            "file_path": path,
            "start_line": start_idx + 1,
            "line_count": end_idx.saturating_sub(start_idx),
            "total_lines": total_lines,
            "complete": complete,
            "content": numbered,
        }))
    }
}

fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        NAME,
        "Read a UTF-8 text file. Use before edit/write when modifying an existing file; partial reads do not authorize edits.",
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "File to read. Relative paths resolve under the goat root." },
                "offset": { "type": "integer", "minimum": 1, "description": "1-based starting line." },
                "limit": { "type": "integer", "minimum": 1, "description": "Maximum lines to return." }
            },
            "required": ["file_path"],
            "additionalProperties": false
        }),
    );
    spec.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "file_path": { "type": "string" },
            "start_line": { "type": "integer" },
            "line_count": { "type": "integer" },
            "total_lines": { "type": "integer" },
            "complete": { "type": "boolean" },
            "content": { "type": "string" }
        },
        "required": ["file_path", "start_line", "line_count", "total_lines", "complete", "content"],
        "additionalProperties": false
    }));
    spec
}

fn ctor() -> Arc<dyn ToolHandler> {
    Arc::new(ReadTool)
}

inventory::submit! {
    ToolFactory { name: NAME, default_enabled: true, spec, ctor }
}

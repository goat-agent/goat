use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use goat_tool::{
    Tool, ToolCall, ToolContext, ToolDefinitionContext, ToolFuture, ToolName, ToolOutput, ToolSpec,
};
use serde::Deserialize;
use serde_json::json;

use crate::agent::common;

pub const NAME: ToolName = ToolName::from_static("edit");

pub struct EditTool;

#[derive(Debug, Deserialize)]
struct EditArgs {
    file_path: PathBuf,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

impl Tool for EditTool {
    fn name(&self) -> ToolName {
        NAME.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Apply an exact text replacement to a file. Requires a complete fresh read of the file first to avoid stale edits.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "old_string": { "type": "string", "description": "Exact text to replace." },
                "new_string": { "type": "string", "description": "Replacement text." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence. Default false requires old_string to be unique." }
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false
        })
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let args = match serde_json::from_value::<EditArgs>(call.arguments.clone()) {
                Ok(args) => args,
                Err(e) => return Ok(common::error(format!("invalid edit input: {e}"))),
            };
            if args.old_string.is_empty() {
                return Ok(common::error("old_string must not be empty"));
            }
            if args.old_string == args.new_string {
                return Ok(common::error("old_string and new_string must differ"));
            }
            let path = match common::existing_path(&ctx, &args.file_path) {
                Ok(path) => path,
                Err(e) => return Ok(common::error(e)),
            };
            let content = match common::read_text(&path) {
                Ok(content) => content,
                Err(e) => return Ok(common::error(e)),
            };
            if let Err(e) = common::require_fresh_complete_read(&ctx, &path, &content) {
                return Ok(common::error(e));
            }
            let count = content.matches(&args.old_string).count();
            if count == 0 {
                return Ok(common::error("old_string not found"));
            }
            if count > 1 && !args.replace_all {
                return Ok(common::error(format!(
                    "old_string appears {count} times; set replace_all=true or provide a more specific old_string"
                )));
            }
            let updated = if args.replace_all {
                content.replace(&args.old_string, &args.new_string)
            } else {
                content.replacen(&args.old_string, &args.new_string, 1)
            };
            if let Err(e) = fs::write(&path, &updated) {
                return Ok(common::error(format!(
                    "cannot write {}: {e}",
                    path.display()
                )));
            }
            if let Err(e) = common::update_complete_snapshot(&ctx, &path, &updated) {
                return Ok(common::error(e));
            }
            Ok(ToolOutput::structured(json!({
                "file_path": path,
                "replacements": if args.replace_all { count } else { 1 },
            })))
        })
    }

    fn definition(&self, _ctx: ToolDefinitionContext) -> Option<ToolSpec> {
        Some(spec())
    }
}

fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        NAME.clone(),
        "Apply an exact text replacement to a file. Requires a complete fresh read of the file first to avoid stale edits.",
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "old_string": { "type": "string", "description": "Exact text to replace." },
                "new_string": { "type": "string", "description": "Replacement text." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence. Default false requires old_string to be unique." }
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false
        }),
    );
    spec.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "file_path": { "type": "string" },
            "replacements": { "type": "integer" }
        },
        "required": ["file_path", "replacements"],
        "additionalProperties": false
    }));
    spec
}

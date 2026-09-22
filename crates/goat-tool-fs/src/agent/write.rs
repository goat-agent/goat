use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use goat_tool::{
    Tool, ToolCall, ToolContext, ToolDefinitionContext, ToolFuture, ToolName, ToolOutput, ToolSpec,
};
use serde::Deserialize;
use serde_json::json;

use crate::agent::common;

pub const NAME: ToolName = ToolName::from_static("write");

pub struct WriteTool;

#[derive(Debug, Deserialize)]
struct WriteArgs {
    file_path: PathBuf,
    content: String,
    #[serde(default)]
    overwrite: bool,
}

impl Tool for WriteTool {
    fn name(&self) -> ToolName {
        NAME.clone()
    }

    fn description(&self) -> Cow<'static, str> {
        "Create a text file, or overwrite an existing text file only after a complete fresh read with overwrite=true.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "content": { "type": "string" },
                "overwrite": { "type": "boolean", "description": "Required for existing files and only allowed after a complete fresh read." }
            },
            "required": ["file_path", "content"],
            "additionalProperties": false
        })
    }

    fn call<'a>(&'a self, call: &'a ToolCall, ctx: ToolContext<'a>) -> ToolFuture<'a> {
        Box::pin(async move {
            let args = match serde_json::from_value::<WriteArgs>(call.arguments.clone()) {
                Ok(args) => args,
                Err(e) => return Ok(common::error(format!("invalid write input: {e}"))),
            };
            let path = match common::writable_path(&ctx, &args.file_path) {
                Ok(path) => path,
                Err(e) => return Ok(common::error(e)),
            };
            let existed = path.exists();
            if existed {
                if !args.overwrite {
                    return Ok(common::error(format!(
                        "file exists; set overwrite=true after a complete read: {}",
                        path.display()
                    )));
                }
                let canonical = match common::existing_path(&ctx, &args.file_path) {
                    Ok(path) => path,
                    Err(e) => return Ok(common::error(e)),
                };
                let old_content = match common::read_text(&canonical) {
                    Ok(content) => content,
                    Err(e) => return Ok(common::error(e)),
                };
                if let Err(e) = common::require_fresh_complete_read(&ctx, &canonical, &old_content)
                {
                    return Ok(common::error(e));
                }
                if let Err(e) = fs::write(&canonical, &args.content) {
                    return Ok(common::error(format!(
                        "cannot write {}: {e}",
                        canonical.display()
                    )));
                }
                if let Err(e) = common::update_complete_snapshot(&ctx, &canonical, &args.content) {
                    return Ok(common::error(e));
                }
                return Ok(ToolOutput::structured(json!({
                    "file_path": canonical,
                    "bytes": args.content.len(),
                    "created": false,
                })));
            }
            if let Some(parent) = path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return Ok(common::error(format!(
                    "cannot create {}: {e}",
                    parent.display()
                )));
            }
            if let Err(e) = fs::write(&path, &args.content) {
                return Ok(common::error(format!(
                    "cannot write {}: {e}",
                    path.display()
                )));
            }
            let canonical = fs::canonicalize(&path).unwrap_or(path);
            if let Err(e) = common::update_complete_snapshot(&ctx, &canonical, &args.content) {
                return Ok(common::error(e));
            }
            Ok(ToolOutput::structured(json!({
                "file_path": canonical,
                "bytes": args.content.len(),
                "created": true,
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
        "Create a text file, or overwrite an existing text file only after a complete fresh read with overwrite=true.",
        json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "content": { "type": "string" },
                "overwrite": { "type": "boolean", "description": "Required for existing files and only allowed after a complete fresh read." }
            },
            "required": ["file_path", "content"],
            "additionalProperties": false
        }),
    );
    spec.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "file_path": { "type": "string" },
            "bytes": { "type": "integer" },
            "created": { "type": "boolean" }
        },
        "required": ["file_path", "bytes", "created"],
        "additionalProperties": false
    }));
    spec
}

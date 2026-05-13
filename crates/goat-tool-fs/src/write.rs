use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use goat_tool::{ToolCall, ToolContext, ToolFactory, ToolHandler, ToolName, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::json;

use crate::common;

pub const NAME: ToolName = ToolName::from_static("write");

pub struct WriteTool;

#[derive(Debug, Deserialize)]
struct WriteArgs {
    file_path: PathBuf,
    content: String,
    #[serde(default)]
    overwrite: bool,
}

#[async_trait]
impl ToolHandler for WriteTool {
    async fn call(&self, ctx: ToolContext, call: ToolCall) -> ToolOutput {
        let args = match serde_json::from_value::<WriteArgs>(call.arguments) {
            Ok(args) => args,
            Err(e) => return common::error(format!("invalid write input: {e}")),
        };
        let path = common::writable_path(&ctx.goat_root, &args.file_path);
        let existed = path.exists();
        if existed {
            if !args.overwrite {
                return common::error(format!(
                    "file exists; set overwrite=true after a complete read: {}",
                    path.display()
                ));
            }
            let canonical = match common::existing_path(&ctx.goat_root, &args.file_path) {
                Ok(path) => path,
                Err(e) => return common::error(e),
            };
            let old_content = match common::read_text(&canonical) {
                Ok(content) => content,
                Err(e) => return common::error(e),
            };
            if let Err(e) = common::require_fresh_complete_read(&ctx, &canonical, &old_content) {
                return common::error(e);
            }
            if let Err(e) = fs::write(&canonical, &args.content) {
                return common::error(format!("cannot write {}: {e}", canonical.display()));
            }
            if let Err(e) = common::update_complete_snapshot(&ctx, &canonical, &args.content) {
                return common::error(e);
            }
            return ToolOutput::structured(json!({
                "file_path": canonical,
                "bytes": args.content.len(),
                "created": false,
            }));
        }
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return common::error(format!("cannot create {}: {e}", parent.display()));
            }
        }
        if let Err(e) = fs::write(&path, &args.content) {
            return common::error(format!("cannot write {}: {e}", path.display()));
        }
        let canonical = fs::canonicalize(&path).unwrap_or(path);
        if let Err(e) = common::update_complete_snapshot(&ctx, &canonical, &args.content) {
            return common::error(e);
        }
        ToolOutput::structured(json!({
            "file_path": canonical,
            "bytes": args.content.len(),
            "created": true,
        }))
    }
}

fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        NAME,
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

fn ctor() -> Arc<dyn ToolHandler> {
    Arc::new(WriteTool)
}

inventory::submit! {
    ToolFactory { name: NAME, default_enabled: true, spec, ctor }
}

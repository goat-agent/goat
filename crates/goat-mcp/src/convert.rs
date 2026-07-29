use goat_tool::{ToolImage, ToolOutput};
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};

use crate::McpError;

pub(crate) fn convert_result(
    tool_name: &str,
    result: CallToolResult,
) -> Result<ToolOutput, McpError> {
    let mut fallback = Vec::new();
    let mut first_image = None;
    for content in result.content {
        match content_to_tool_content(content) {
            ToolResultPart::Text(text) => fallback.push(text),
            ToolResultPart::Image(image) => {
                if first_image.is_none() {
                    first_image = Some(image);
                }
            }
        }
    }
    if let Some(value) = result.structured_content {
        fallback.push(format!("structuredContent: {value}"));
    }
    if result.is_error.unwrap_or(false) {
        let message = if fallback.is_empty() {
            "MCP tool returned an error".to_owned()
        } else {
            fallback.join("\n")
        };
        return Err(McpError::ToolError {
            tool: tool_name.to_owned(),
            message,
        });
    }
    if !fallback.is_empty() {
        Ok(ToolOutput::text(fallback.join("\n")).with_summary(summary(&fallback)))
    } else if let Some(image) = first_image {
        Ok(ToolOutput::image(image).with_summary("image"))
    } else {
        Ok(ToolOutput::text(String::new()))
    }
}

enum ToolResultPart {
    Text(String),
    Image(ToolImage),
}

fn content_to_tool_content(content: ContentBlock) -> ToolResultPart {
    match content {
        ContentBlock::Text(text) => ToolResultPart::Text(text.text),
        ContentBlock::Image(image) => ToolResultPart::Image(ToolImage {
            media_type: image.mime_type,
            data: image.data,
        }),
        ContentBlock::Audio(audio) => ToolResultPart::Text(format!(
            "audio result: mimeType={}, base64Bytes={}",
            audio.mime_type,
            audio.data.len()
        )),
        ContentBlock::Resource(resource) => {
            ToolResultPart::Text(resource_fallback(resource.resource))
        }
        ContentBlock::ResourceLink(resource) => ToolResultPart::Text(format!(
            "resource link: uri={}, name={}",
            resource.uri, resource.name
        )),
        other => ToolResultPart::Text(render_unknown("content block", &other)),
    }
}

fn resource_fallback(resource: ResourceContents) -> String {
    match resource {
        ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            ..
        } => format!(
            "embedded resource: uri={}, mimeType={}\n{}",
            uri,
            mime_type.unwrap_or_default(),
            text
        ),
        ResourceContents::BlobResourceContents {
            uri,
            mime_type,
            blob,
            ..
        } => format!(
            "embedded resource: uri={}, mimeType={}, base64Bytes={}",
            uri,
            mime_type.unwrap_or_default(),
            blob.len()
        ),
        ref other => render_unknown("resource contents", other),
    }
}

fn render_unknown<T: serde::Serialize>(kind: &str, value: &T) -> String {
    serde_json::to_string(value).map_or_else(
        |_| format!("unrecognized {kind}"),
        |json| format!("unrecognized {kind}: {json}"),
    )
}

fn summary(parts: &[String]) -> String {
    parts
        .iter()
        .find_map(|part| part.lines().find(|line| !line.trim().is_empty()))
        .map_or_else(String::new, |line| line.chars().take(80).collect())
}

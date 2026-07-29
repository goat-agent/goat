use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpImage {
    pub media_type: String,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpContent {
    Text(String),
    Image(McpImage),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    pub structured: Option<Value>,
    pub is_error: bool,
}

impl From<CallToolResult> for McpToolResult {
    fn from(result: CallToolResult) -> Self {
        Self {
            content: result.content.into_iter().map(McpContent::from).collect(),
            structured: result.structured_content,
            is_error: result.is_error.unwrap_or(false),
        }
    }
}

impl McpToolResult {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|part| match part {
                McpContent::Text(text) => Some(text.as_str()),
                McpContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn first_image(&self) -> Option<&McpImage> {
        self.content.iter().find_map(|part| match part {
            McpContent::Image(image) => Some(image),
            McpContent::Text(_) => None,
        })
    }

    pub fn value(&self) -> Value {
        if let Some(structured) = &self.structured {
            return structured.clone();
        }
        let joined = self.text();
        serde_json::from_str(&joined).unwrap_or(Value::String(joined))
    }

    pub fn error_message(&self) -> String {
        let text = self.text();
        if text.trim().is_empty() {
            "no error detail".to_owned()
        } else {
            text
        }
    }
}

impl From<ContentBlock> for McpContent {
    fn from(block: ContentBlock) -> Self {
        match block {
            ContentBlock::Text(text) => Self::Text(text.text),
            ContentBlock::Image(image) => Self::Image(McpImage {
                media_type: image.mime_type,
                data: image.data,
            }),
            ContentBlock::Audio(audio) => Self::Text(format!(
                "audio result: mimeType={}, base64Bytes={}",
                audio.mime_type,
                audio.data.len()
            )),
            ContentBlock::Resource(resource) => Self::Text(render_resource(&resource.resource)),
            ContentBlock::ResourceLink(resource) => Self::Text(format!(
                "resource link: uri={}, name={}",
                resource.uri, resource.name
            )),
            ref other => Self::Text(render_unknown("content block", other)),
        }
    }
}

fn render_resource(resource: &ResourceContents) -> String {
    match resource {
        ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            ..
        } => format!(
            "embedded resource: uri={}, mimeType={}\n{}",
            uri,
            mime_type.clone().unwrap_or_default(),
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
            mime_type.clone().unwrap_or_default(),
            blob.len()
        ),
        other => render_unknown("resource contents", other),
    }
}

fn render_unknown<T: serde::Serialize>(kind: &str, value: &T) -> String {
    serde_json::to_string(value).map_or_else(
        |_| format!("unrecognized {kind}"),
        |json| format!("unrecognized {kind}: {json}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structured_content_wins_over_text() {
        let result = McpToolResult::from(CallToolResult::structured(json!({"ok": true})));
        assert_eq!(result.value(), json!({"ok": true}));
    }

    #[test]
    fn bare_text_that_parses_becomes_json() {
        let result = McpToolResult::from(CallToolResult::success(vec![ContentBlock::text(
            "{\"n\":1}",
        )]));
        assert_eq!(result.value(), json!({"n": 1}));
    }

    #[test]
    fn bare_text_that_does_not_parse_stays_a_string() {
        let result =
            McpToolResult::from(CallToolResult::success(vec![ContentBlock::text("hello")]));
        assert_eq!(result.value(), json!("hello"));
    }

    #[test]
    fn images_survive_alongside_text() {
        let result = McpToolResult::from(CallToolResult::success(vec![
            ContentBlock::text("caption"),
            ContentBlock::image("YmFzZTY0", "image/png"),
        ]));
        assert_eq!(result.text(), "caption");
        let image = result.first_image().expect("image kept");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.data, "YmFzZTY0");
    }

    #[test]
    fn audio_and_resources_degrade_to_text() {
        let result = McpToolResult::from(CallToolResult::success(vec![
            ContentBlock::audio("YQ", "audio/wav"),
            ContentBlock::embedded_text("file:///a", "body"),
        ]));
        let text = result.text();
        assert!(text.contains("audio result: mimeType=audio/wav"));
        assert!(text.contains("embedded resource: uri=file:///a"));
        assert!(text.contains("body"));
    }

    #[test]
    fn error_results_report_their_text() {
        let result = McpToolResult::from(CallToolResult::error(vec![ContentBlock::text("bad")]));
        assert!(result.is_error);
        assert_eq!(result.error_message(), "bad");
    }

    #[test]
    fn empty_error_results_still_say_something() {
        let result = McpToolResult::from(CallToolResult::error(Vec::new()));
        assert_eq!(result.error_message(), "no error detail");
    }
}

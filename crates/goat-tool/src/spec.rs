use serde::Serialize;

use crate::name::ToolName;

#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    pub name: ToolName,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub annotations: serde_json::Value,
}

impl ToolSpec {
    pub fn new(
        name: ToolName,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name,
            title: None,
            description: Some(description.into()),
            input_schema,
            output_schema: None,
            annotations: serde_json::Value::Null,
        }
    }
}

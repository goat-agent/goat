use serde::{Deserialize, Serialize};

use crate::Model;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ContentPart {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        id: String,
        name: String,
        content: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

impl LlmMessage {
    pub fn user_text(s: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::Text(s.into())],
        }
    }

    pub fn assistant_text(s: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::Text(s.into())],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct LlmRequest {
    pub model: Model,
    pub system: Option<String>,
    pub messages: Vec<LlmMessage>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub tools: Vec<ToolSpec>,
}

impl LlmRequest {
    pub fn new(model: Model) -> Self {
        Self {
            model,
            system: None,
            messages: vec![],
            max_tokens: 2048,
            temperature: None,
            tools: vec![],
        }
    }
}

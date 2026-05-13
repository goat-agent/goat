use goat_llm::{ContentPart, LlmMessage, LlmRequest, Role, ToolSpec};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Body<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<Message>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
}

#[derive(Serialize)]
struct Message {
    role: &'static str,
    content: Vec<Content>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize)]
struct Tool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

impl<'a> From<&'a LlmRequest> for Body<'a> {
    fn from(req: &'a LlmRequest) -> Self {
        let messages = req
            .messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .map(message_from)
            .collect();
        Body {
            model: req.model.id(),
            max_tokens: req.max_tokens,
            messages,
            stream: true,
            system: req.system.as_deref(),
            temperature: req.temperature,
            tools: req.tools.iter().map(tool_from).collect(),
        }
    }
}

fn tool_from(spec: &ToolSpec) -> Tool {
    Tool {
        name: spec.name.clone(),
        description: spec.description.clone(),
        input_schema: spec.input_schema.clone(),
    }
}

fn message_from(m: &LlmMessage) -> Message {
    let role = match m.role {
        Role::Assistant => "assistant",
        _ => "user",
    };
    let content = m
        .content
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(Content::Text { text: t.clone() }),
            ContentPart::ToolCall {
                id,
                name,
                arguments,
            } => Some(Content::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: arguments.clone(),
            }),
            ContentPart::ToolResult { id, content, .. } => Some(Content::ToolResult {
                tool_use_id: id.clone(),
                content: content.clone(),
            }),
            _ => None,
        })
        .collect();
    Message { role, content }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_llm::{LlmMessage, LlmRequest, Model};
    use serde_json::json;

    #[test]
    fn body_includes_system_and_temperature() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "claude-x"),
            system: Some("be kind".into()),
            messages: vec![LlmMessage::user_text("hi")],
            max_tokens: 64,
            temperature: Some(0.5),
            tools: vec![],
        };
        let body = Body::from(&req);
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["model"], "claude-x");
        assert_eq!(v["max_tokens"], 64);
        assert_eq!(v["stream"], true);
        assert_eq!(v["system"], "be kind");
        assert_eq!(v["temperature"], 0.5);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"][0]["type"], "text");
        assert_eq!(v["messages"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn body_omits_optional_fields() {
        let req = LlmRequest::new(Model::new(crate::ID, "claude-x"));
        let body = Body::from(&req);
        let v = serde_json::to_value(&body).unwrap();
        assert!(v.get("system").is_none());
        assert!(v.get("temperature").is_none());
    }

    #[test]
    fn body_drops_system_role_messages() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "claude-x"),
            system: None,
            messages: vec![
                LlmMessage {
                    role: Role::System,
                    content: vec![ContentPart::Text("filtered".into())],
                },
                LlmMessage::user_text("kept"),
            ],
            max_tokens: 8,
            temperature: None,
            tools: vec![],
        };
        let body = Body::from(&req);
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
        assert_eq!(v["messages"][0]["content"][0]["text"], "kept");
    }

    #[test]
    fn body_encodes_tools_and_tool_use() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "claude-x"),
            system: None,
            messages: vec![
                LlmMessage {
                    role: Role::Assistant,
                    content: vec![ContentPart::ToolCall {
                        id: "call_1".into(),
                        name: "shell".into(),
                        arguments: json!({"command":"pwd"}),
                    }],
                },
                LlmMessage {
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        id: "call_1".into(),
                        name: "shell".into(),
                        content: "ok".into(),
                    }],
                },
            ],
            max_tokens: 8,
            temperature: None,
            tools: vec![ToolSpec {
                name: "shell".into(),
                description: "Run a command".into(),
                input_schema: json!({"type":"object"}),
            }],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["tools"][0]["name"], "shell");
        assert_eq!(v["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(v["messages"][0]["content"][0]["name"], "shell");
        assert_eq!(v["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(v["messages"][1]["content"][0]["tool_use_id"], "call_1");
    }
}

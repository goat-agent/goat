use goat_llm::{ContentPart, LlmMessage, LlmRequest, Role, ToolSpec};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Body<'a> {
    model: &'a str,
    messages: Vec<Message>,
    stream: bool,
    stream_options: StreamOptions,
    max_completion_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct Message {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ToolCall>,
}

#[derive(Serialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionCall,
}

#[derive(Serialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionTool,
}

#[derive(Serialize)]
struct FunctionTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl<'a> From<&'a LlmRequest> for Body<'a> {
    fn from(req: &'a LlmRequest) -> Self {
        let mut messages = Vec::with_capacity(req.messages.len() + 1);
        if let Some(sys) = req.system.as_deref() {
            messages.push(Message {
                role: "system",
                content: Some(sys.to_string()),
                tool_call_id: None,
                tool_calls: vec![],
            });
        }
        for m in &req.messages {
            messages.push(message_from(m));
        }
        Body {
            model: req.model.id(),
            messages,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            max_completion_tokens: req.max_tokens,
            temperature: req.temperature.map(|t| t.clamp(0.0, 1.0)),
            tools: req.tools.iter().map(tool_from).collect(),
        }
    }
}

fn tool_from(spec: &ToolSpec) -> Tool {
    Tool {
        kind: "function",
        function: FunctionTool {
            name: spec.name.clone(),
            description: spec.description.clone(),
            parameters: spec.input_schema.clone(),
        },
    }
}

fn message_from(m: &LlmMessage) -> Message {
    if matches!(m.role, Role::Tool) {
        if let Some(ContentPart::ToolResult { id, content, .. }) = m.content.first() {
            return Message {
                role: "tool",
                content: Some(content.clone()),
                tool_call_id: Some(id.clone()),
                tool_calls: vec![],
            };
        }
    }

    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    };
    let content = text_content(m);
    let tool_calls: Vec<ToolCall> = m
        .content
        .iter()
        .filter_map(|p| match p {
            ContentPart::ToolCall {
                id,
                name,
                arguments,
            } => Some(ToolCall {
                id: id.clone(),
                kind: "function",
                function: FunctionCall {
                    name: name.clone(),
                    arguments: arguments.to_string(),
                },
            }),
            _ => None,
        })
        .collect();
    let content = if role == "assistant" && !tool_calls.is_empty() && content.is_none() {
        Some(String::new())
    } else {
        content
    };
    Message {
        role,
        content,
        tool_call_id: None,
        tool_calls,
    }
}

fn text_content(m: &LlmMessage) -> Option<String> {
    let text = m
        .content
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_llm::{LlmMessage, LlmRequest, Model};
    use serde_json::json;

    #[test]
    fn body_clamps_high_temperature_to_one() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "kimi-x"),
            system: None,
            messages: vec![LlmMessage::user_text("hi")],
            max_tokens: 32,
            temperature: Some(1.75),
            tools: vec![],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["temperature"], 1.0);
    }

    #[test]
    fn body_clamps_negative_temperature_to_zero() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "kimi-x"),
            system: None,
            messages: vec![LlmMessage::user_text("hi")],
            max_tokens: 32,
            temperature: Some(-0.5),
            tools: vec![],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["temperature"], 0.0);
    }

    #[test]
    fn body_uses_max_completion_tokens_and_tools() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "kimi-x"),
            system: Some("be brief".into()),
            messages: vec![LlmMessage {
                role: Role::Assistant,
                content: vec![ContentPart::ToolCall {
                    id: "call_1".into(),
                    name: "echo".into(),
                    arguments: json!({"x": 1}),
                }],
            }],
            max_tokens: 64,
            temperature: Some(0.5),
            tools: vec![ToolSpec {
                name: "echo".into(),
                description: "Echo".into(),
                input_schema: json!({"type":"object"}),
            }],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["max_completion_tokens"], 64);
        assert!(v.get("max_tokens").is_none());
        assert_eq!(v["temperature"], 0.5);
        assert_eq!(v["stream_options"]["include_usage"], true);
        assert_eq!(v["tools"][0]["function"]["name"], "echo");
        assert_eq!(
            v["messages"][1]["tool_calls"][0]["function"]["name"],
            "echo"
        );
    }
}

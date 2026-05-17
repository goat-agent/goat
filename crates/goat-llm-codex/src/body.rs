use goat_llm::{ContentPart, LlmMessage, LlmRequest, Role, ToolSpec};
use serde::Serialize;

/// Body for `POST /backend-api/codex/responses` — the Responses API as
/// served by the ChatGPT Codex backend. This is a restricted surface
/// compared with the public OpenAI Responses API and rejects client-side
/// sampling/output controls outright.
///
/// Contract notes that the backend enforces with HTTP 400:
/// - `temperature`, `max_tokens`, `max_output_tokens`, `metadata`,
///   `user`, `context_management` are NOT accepted. We omit them.
/// - `instructions` carries the single system prompt; `system`-role
///   messages are NOT echoed into `input` (the backend would reject the
///   duplicate).
/// - User content uses `input_text`; assistant content uses `output_text`.
///   Mixing them yields `invalid_value` errors.
/// - Assistant tool calls and tool results are first-class items in
///   `input` (`function_call`, `function_call_output`) rather than
///   nested in messages.
/// - `store: false` is kept mandatory so the backend doesn't persist the
///   conversation upstream.
#[derive(Serialize)]
pub(crate) struct Body<'a> {
    model: &'a str,
    instructions: String,
    input: Vec<InputItem>,
    stream: bool,
    store: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum InputItem {
    Message {
        role: &'static str,
        content: Vec<ContentItem>,
    },
    FunctionCall {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: String,
        output: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ContentItem {
    /// Used for `user` content per the Responses API contract.
    #[serde(rename = "input_text")]
    InputText { text: String },
    /// Required by the Responses API for `assistant` role content. The
    /// API rejects `input_text` in assistant items with an
    /// `invalid_value` error.
    #[serde(rename = "output_text")]
    OutputText { text: String },
}

#[derive(Serialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl<'a> From<&'a LlmRequest> for Body<'a> {
    fn from(req: &'a LlmRequest) -> Self {
        let instructions = req.system.clone().unwrap_or_default();
        let input = req.messages.iter().flat_map(input_items_from).collect();
        Body {
            model: req.model.id(),
            instructions,
            input,
            stream: true,
            store: false,
            tools: req.tools.iter().map(tool_from).collect(),
        }
    }
}

fn input_items_from(m: &LlmMessage) -> Vec<InputItem> {
    let mut out = Vec::new();
    match m.role {
        Role::System => {
            // `instructions` carries the system prompt; do not duplicate.
        }
        Role::Tool => {
            for part in &m.content {
                if let ContentPart::ToolResult { id, content, .. } = part {
                    out.push(InputItem::FunctionCallOutput {
                        kind: "function_call_output",
                        call_id: id.clone(),
                        output: content.clone(),
                    });
                }
            }
        }
        Role::User => {
            if let Some(text) = extract_text(&m.content) {
                out.push(InputItem::Message {
                    role: "user",
                    content: vec![ContentItem::InputText { text }],
                });
            }
        }
        Role::Assistant => {
            for part in &m.content {
                if let ContentPart::ToolCall {
                    id,
                    name,
                    arguments,
                } = part
                {
                    out.push(InputItem::FunctionCall {
                        kind: "function_call",
                        call_id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.to_string(),
                    });
                }
            }
            if let Some(text) = extract_text(&m.content) {
                out.push(InputItem::Message {
                    role: "assistant",
                    content: vec![ContentItem::OutputText { text }],
                });
            }
        }
    }
    out
}

fn extract_text(parts: &[ContentPart]) -> Option<String> {
    let text = parts
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

fn tool_from(spec: &ToolSpec) -> Tool {
    Tool {
        kind: "function",
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters: spec.input_schema.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_llm::{LlmMessage, LlmRequest, Model};
    use serde_json::json;

    fn base_req() -> LlmRequest {
        LlmRequest {
            model: Model::new(crate::ID, "gpt-5.2-codex"),
            system: Some("act briefly".into()),
            messages: vec![],
            max_tokens: 100,
            temperature: None,
            tools: vec![],
        }
    }

    #[test]
    fn user_message_is_input_text() {
        let mut req = base_req();
        req.messages = vec![LlmMessage::user_text("hello")];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["model"], "gpt-5.2-codex");
        assert_eq!(v["instructions"], "act briefly");
        assert_eq!(v["store"], false);
        assert_eq!(v["stream"], true);
        assert_eq!(v["input"][0]["role"], "user");
        assert_eq!(v["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(v["input"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn assistant_message_is_output_text() {
        let mut req = base_req();
        req.messages = vec![LlmMessage::assistant_text("done")];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["input"][0]["role"], "assistant");
        assert_eq!(v["input"][0]["content"][0]["type"], "output_text");
        assert_eq!(v["input"][0]["content"][0]["text"], "done");
    }

    #[test]
    fn assistant_tool_call_becomes_function_call_item() {
        let mut req = base_req();
        req.messages = vec![LlmMessage {
            role: Role::Assistant,
            content: vec![ContentPart::ToolCall {
                id: "call_42".into(),
                name: "schedule_once".into(),
                arguments: json!({"due_at": "2026-05-17T23:00:00Z"}),
            }],
        }];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["input"][0]["type"], "function_call");
        assert_eq!(v["input"][0]["call_id"], "call_42");
        assert_eq!(v["input"][0]["name"], "schedule_once");
        let args = v["input"][0]["arguments"].as_str().unwrap();
        assert!(args.contains("2026-05-17T23:00:00Z"));
    }

    #[test]
    fn tool_role_becomes_function_call_output_item() {
        let mut req = base_req();
        req.messages = vec![LlmMessage {
            role: Role::Tool,
            content: vec![ContentPart::ToolResult {
                id: "call_42".into(),
                name: "schedule_once".into(),
                content: r#"{"task_id":5}"#.into(),
            }],
        }];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["input"][0]["type"], "function_call_output");
        assert_eq!(v["input"][0]["call_id"], "call_42");
        assert_eq!(v["input"][0]["output"], r#"{"task_id":5}"#);
    }

    #[test]
    fn assistant_tool_call_then_text_emits_two_items() {
        let mut req = base_req();
        req.messages = vec![LlmMessage {
            role: Role::Assistant,
            content: vec![
                ContentPart::ToolCall {
                    id: "c1".into(),
                    name: "list_tasks".into(),
                    arguments: json!({}),
                },
                ContentPart::Text("done".into()),
            ],
        }];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        let input = v["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
    }

    #[test]
    fn system_role_message_is_dropped_from_input() {
        let mut req = base_req();
        req.messages = vec![
            LlmMessage {
                role: Role::System,
                content: vec![ContentPart::Text("ignored system".into())],
            },
            LlmMessage::user_text("hello"),
        ];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        let input = v["input"].as_array().unwrap();
        assert_eq!(input.len(), 1, "system message must not appear in input");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(v["instructions"], "act briefly");
    }

    #[test]
    fn sampling_parameters_are_never_serialised() {
        // ChatGPT Codex backend rejects temperature, max_tokens, and
        // max_output_tokens with HTTP 400. We must omit them regardless
        // of what the caller set on the LlmRequest.
        let mut req = base_req();
        req.temperature = Some(0.5);
        req.max_tokens = 256;
        req.messages = vec![LlmMessage::user_text("hi")];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert!(v.get("temperature").is_none());
        assert!(v.get("max_tokens").is_none());
        assert!(v.get("max_output_tokens").is_none());
    }

    #[test]
    fn tool_round_trip_preserves_call_id_link() {
        let mut req = base_req();
        req.messages = vec![
            LlmMessage::user_text("schedule something"),
            LlmMessage {
                role: Role::Assistant,
                content: vec![ContentPart::ToolCall {
                    id: "abc".into(),
                    name: "schedule_once".into(),
                    arguments: json!({"due_at": "2026-05-17T23:00:00Z"}),
                }],
            },
            LlmMessage {
                role: Role::Tool,
                content: vec![ContentPart::ToolResult {
                    id: "abc".into(),
                    name: "schedule_once".into(),
                    content: r#"{"task_id":1}"#.into(),
                }],
            },
        ];
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        let input = v["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "abc");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "abc");
    }
}

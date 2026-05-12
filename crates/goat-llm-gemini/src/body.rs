use goat_llm::{ContentPart, LlmMessage, LlmRequest, Role, ToolSpec};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Body<'a> {
    contents: Vec<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<SystemInstruction<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
}

#[derive(Serialize)]
struct GenerationConfig {
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct SystemInstruction<'a> {
    parts: Vec<TextPart<'a>>,
}

#[derive(Serialize)]
struct Content {
    role: &'static str,
    parts: Vec<Part>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Part {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: FunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: FunctionResponse,
    },
}

#[derive(Serialize)]
struct TextPart<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct FunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize)]
struct FunctionResponse {
    name: String,
    response: FunctionResponseBody,
}

#[derive(Serialize)]
struct FunctionResponseBody {
    content: String,
}

#[derive(Serialize)]
struct Tool {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Serialize)]
struct FunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl<'a> From<&'a LlmRequest> for Body<'a> {
    fn from(req: &'a LlmRequest) -> Self {
        let contents = req.messages.iter().map(message_from).collect();
        Body {
            contents,
            generation_config: GenerationConfig {
                max_output_tokens: req.max_tokens,
                temperature: req.temperature,
            },
            system_instruction: req.system.as_deref().map(|s| SystemInstruction {
                parts: vec![TextPart { text: s }],
            }),
            tools: tools_from(&req.tools),
        }
    }
}

fn tools_from(specs: &[ToolSpec]) -> Vec<Tool> {
    if specs.is_empty() {
        return vec![];
    }
    vec![Tool {
        function_declarations: specs
            .iter()
            .map(|spec| FunctionDeclaration {
                name: spec.name.clone(),
                description: spec.description.clone(),
                parameters: spec.input_schema.clone(),
            })
            .collect(),
    }]
}

fn message_from(m: &LlmMessage) -> Content {
    let role = match m.role {
        Role::Assistant => "model",
        _ => "user",
    };
    let parts = m
        .content
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text(t) => Some(Part::Text { text: t.clone() }),
            ContentPart::ToolCall {
                name, arguments, ..
            } => Some(Part::FunctionCall {
                function_call: FunctionCall {
                    name: name.clone(),
                    args: arguments.clone(),
                },
            }),
            ContentPart::ToolResult { name, content, .. } => Some(Part::FunctionResponse {
                function_response: FunctionResponse {
                    name: name.clone(),
                    response: FunctionResponseBody {
                        content: content.clone(),
                    },
                },
            }),
            _ => None,
        })
        .collect();
    Content { role, parts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goat_llm::{LlmMessage, LlmRequest, Model};
    use serde_json::json;

    #[test]
    fn body_lifts_system_to_system_instruction() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "gemini-x"),
            system: Some("be calm".into()),
            messages: vec![LlmMessage::user_text("hi")],
            max_tokens: 32,
            temperature: Some(0.5),
            tools: vec![],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["systemInstruction"]["parts"][0]["text"], "be calm");
        assert_eq!(v["generationConfig"]["maxOutputTokens"], 32);
        assert_eq!(v["generationConfig"]["temperature"], 0.5);
        assert_eq!(v["contents"][0]["role"], "user");
        assert_eq!(v["contents"][0]["parts"][0]["text"], "hi");
    }

    #[test]
    fn body_maps_assistant_role_to_model() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "gemini-x"),
            system: None,
            messages: vec![LlmMessage::assistant_text("ok")],
            max_tokens: 8,
            temperature: None,
            tools: vec![],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["contents"][0]["role"], "model");
        assert!(v.get("systemInstruction").is_none());
        assert!(v["generationConfig"].get("temperature").is_none());
    }

    #[test]
    fn body_encodes_tool_protocol() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "gemini-x"),
            system: None,
            messages: vec![
                LlmMessage {
                    role: Role::Assistant,
                    content: vec![ContentPart::ToolCall {
                        id: "call_1".into(),
                        name: "lookup".into(),
                        arguments: json!({"q":"x"}),
                    }],
                },
                LlmMessage {
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        id: "call_1".into(),
                        name: "lookup".into(),
                        content: "42".into(),
                    }],
                },
            ],
            max_tokens: 8,
            temperature: None,
            tools: vec![ToolSpec {
                name: "lookup".into(),
                description: "Lookup".into(),
                input_schema: json!({"type":"object"}),
            }],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["tools"][0]["functionDeclarations"][0]["name"], "lookup");
        assert_eq!(
            v["contents"][0]["parts"][0]["functionCall"]["name"],
            "lookup"
        );
        let part = &v["contents"][1]["parts"][0];
        assert_eq!(part["functionResponse"]["name"], "lookup");
        assert_eq!(part["functionResponse"]["response"]["content"], "42");
    }
}

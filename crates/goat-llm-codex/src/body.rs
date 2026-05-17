use goat_llm::{ContentPart, LlmMessage, LlmRequest, Role, ToolSpec};
use serde::Serialize;

/// Body for `POST /backend-api/codex/responses` — Responses API with WHAM constraints:
/// `instructions` required, content type must be `input_text`, `store: false` mandatory.
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
struct InputItem {
    role: &'static str,
    content: Vec<ContentItem>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ContentItem {
    #[serde(rename = "input_text")]
    InputText { text: String },
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
        let input = req.messages.iter().filter_map(input_item_from).collect();
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

fn input_item_from(m: &LlmMessage) -> Option<InputItem> {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => return None,
    };
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
        return None;
    }
    Some(InputItem {
        role,
        content: vec![ContentItem::InputText { text }],
    })
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

    #[test]
    fn body_obeys_wham_constraints() {
        let req = LlmRequest {
            model: Model::new(crate::ID, "gpt-5.2-codex"),
            system: Some("act briefly".into()),
            messages: vec![LlmMessage::user_text("hello")],
            max_tokens: 100,
            temperature: None,
            tools: vec![],
        };
        let v = serde_json::to_value(Body::from(&req)).unwrap();
        assert_eq!(v["model"], "gpt-5.2-codex");
        assert_eq!(v["instructions"], "act briefly");
        assert_eq!(v["store"], false);
        assert_eq!(v["stream"], true);
        assert_eq!(v["input"][0]["role"], "user");
        assert_eq!(v["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(v["input"][0]["content"][0]["text"], "hello");
    }
}

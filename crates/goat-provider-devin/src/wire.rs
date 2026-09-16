use goat_provider::{ContentBlock, Message, MessageRole, Request, ToolChoice, Usage};
use uuid::Uuid;

use crate::client;
use crate::proto::{self, Reader, Value, as_str, as_u64};

const SOURCE_USER: u64 = 1;
const SOURCE_SYSTEM: u64 = 2;
const SOURCE_TOOL: u64 = 4;

const REQUEST_TYPE_CASCADE: u64 = 5;
const PLANNER_MODE_DEFAULT: u64 = 1;
const CACHE_EPHEMERAL: u64 = 1;

const DEFAULT_STOP_PATTERNS: &[&str] = &[
    "<|user|>",
    "<|bot|>",
    "<|context_request|>",
    "<|endoftext|>",
    "<|end_of_turn|>",
];

fn prompt_id(cascade_id: &str, index: usize, tag: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("{cascade_id}\0{index}\0{tag}").as_bytes(),
    )
    .to_string()
}

fn tool_call_bytes(id: &str, name: &str, arguments_json: &str) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_str(&mut out, 1, id);
    proto::field_str(&mut out, 2, name);
    proto::field_str(&mut out, 3, arguments_json);
    out
}

fn image_bytes(media_type: &str, data: &str) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_str(&mut out, 1, data);
    proto::field_str(&mut out, 2, media_type);
    out
}

fn user_prompt(cascade_id: &str, index: usize, text: &str, images: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_str(&mut out, 1, &prompt_id(cascade_id, index, "user"));
    proto::field_varint(&mut out, 2, SOURCE_USER);
    proto::field_str(&mut out, 3, text);
    for image in images {
        proto::field_msg(&mut out, 10, image);
    }
    out
}

fn tool_prompt(
    cascade_id: &str,
    index: usize,
    tool_call_id: &str,
    text: &str,
    is_error: bool,
    images: &[Vec<u8>],
) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_str(
        &mut out,
        1,
        &prompt_id(cascade_id, index, &format!("tool\0{tool_call_id}")),
    );
    proto::field_varint(&mut out, 2, SOURCE_TOOL);
    proto::field_str(&mut out, 3, text);
    proto::field_str(&mut out, 7, tool_call_id);
    if is_error {
        proto::field_bool(&mut out, 9, true);
    }
    for image in images {
        proto::field_msg(&mut out, 10, image);
    }
    out
}

fn assistant_prompt(
    cascade_id: &str,
    index: usize,
    text: &str,
    thinking: &str,
    signature: &str,
    tool_calls: &[Vec<u8>],
) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_str(
        &mut out,
        1,
        &format!("bot-{}", prompt_id(cascade_id, index, "assistant")),
    );
    proto::field_varint(&mut out, 2, SOURCE_SYSTEM);
    proto::field_str(&mut out, 3, text);
    for call in tool_calls {
        proto::field_msg(&mut out, 6, call);
    }
    proto::field_str(&mut out, 11, thinking);
    proto::field_str(&mut out, 12, signature);
    out
}

fn split_user_parts(content: &[ContentBlock]) -> (String, Vec<Vec<u8>>, Vec<&ContentBlock>) {
    let mut text = String::new();
    let mut images = Vec::new();
    let mut results = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::Image { media_type, data } => {
                images.push(image_bytes(media_type, data));
            }
            ContentBlock::ToolResult { .. } => results.push(block),
            _ => {}
        }
    }
    (text, images, results)
}

fn tool_result_parts(content: &[ContentBlock]) -> (String, Vec<Vec<u8>>) {
    let mut text = String::new();
    let mut images = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::Image { media_type, data } => {
                images.push(image_bytes(media_type, data));
            }
            _ => {}
        }
    }
    (text, images)
}

fn encode_messages(messages: &[Message], cascade_id: &str) -> Vec<Vec<u8>> {
    let mut prompts = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        match message.role {
            MessageRole::System => {}
            MessageRole::User => {
                let (text, images, results) = split_user_parts(&message.content);
                if !text.is_empty() || !images.is_empty() || results.is_empty() {
                    push_prompt_into(&mut prompts, user_prompt(cascade_id, index, &text, &images));
                }
                for result in results {
                    let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = result
                    else {
                        continue;
                    };
                    let (result_text, result_images) = tool_result_parts(content);
                    push_prompt_into(
                        &mut prompts,
                        tool_prompt(
                            cascade_id,
                            index,
                            tool_use_id,
                            &result_text,
                            *is_error,
                            &result_images,
                        ),
                    );
                }
            }
            MessageRole::Assistant => {
                let mut text = String::new();
                let mut thinking = String::new();
                let mut signature = String::new();
                let mut tool_calls = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text: t } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        ContentBlock::Thinking {
                            text: t,
                            signature: s,
                        } => {
                            if !thinking.is_empty() {
                                thinking.push('\n');
                            }
                            thinking.push_str(t);
                            if signature.is_empty() {
                                signature.clone_from(s);
                            }
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(tool_call_bytes(
                                id,
                                name,
                                &serde_json::to_string(input).unwrap_or_else(|_| "{}".to_owned()),
                            ));
                        }
                        _ => {}
                    }
                }
                push_prompt_into(
                    &mut prompts,
                    assistant_prompt(cascade_id, index, &text, &thinking, &signature, &tool_calls),
                );
            }
        }
    }
    prompts
}

fn push_prompt_into(prompts: &mut Vec<Vec<u8>>, prompt: Vec<u8>) {
    prompts.push(prompt);
}

fn configuration(req: &Request) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_varint(&mut out, 1, 1);
    proto::field_varint(&mut out, 2, u64::from(req.max_tokens.unwrap_or(64_000)));
    proto::field_varint(&mut out, 3, 200);
    let temperature = f64::from(req.temperature.unwrap_or(0.4));
    proto::field_double(&mut out, 5, temperature);
    proto::field_double(&mut out, 6, temperature);
    proto::field_varint(&mut out, 7, 50);
    proto::field_double(&mut out, 8, 1.0);
    for pattern in DEFAULT_STOP_PATTERNS {
        proto::field_str(&mut out, 9, pattern);
    }
    proto::field_double(&mut out, 11, 1.0);
    out
}

fn tool_definition(name: &str, description: &str, schema: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::new();
    proto::field_str(&mut out, 1, name);
    proto::field_str(&mut out, 2, description);
    proto::field_str(
        &mut out,
        3,
        &serde_json::to_string(schema).unwrap_or_else(|_| "{}".to_owned()),
    );
    out
}

pub fn chat_request(req: &Request, api_key: &str, user_jwt: &str, session_id: &str) -> Vec<u8> {
    let cascade_id = session_id.to_owned();
    let mut out = Vec::new();
    proto::field_msg(
        &mut out,
        1,
        &client::metadata(api_key, Some(user_jwt), session_id),
    );
    let mut system = req.system.clone().unwrap_or_default();
    for message in &req.messages {
        if message.role == MessageRole::System {
            let text = message.text_content();
            if !text.is_empty() {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&text);
            }
        }
    }
    proto::field_str(&mut out, 2, &system);
    for prompt in encode_messages(&req.messages, &cascade_id) {
        proto::field_msg(&mut out, 3, &prompt);
    }
    proto::field_varint(&mut out, 7, REQUEST_TYPE_CASCADE);
    proto::field_msg(&mut out, 8, &configuration(req));
    for tool in &req.tools {
        proto::field_msg(
            &mut out,
            10,
            &tool_definition(&tool.name, &tool.description, &tool.input_schema),
        );
    }
    proto::field_bool(&mut out, 11, true);
    let choice = match req.tool_choice {
        ToolChoice::Auto => "auto",
        ToolChoice::None => "none",
    };
    let mut choice_msg = Vec::new();
    proto::field_str(&mut choice_msg, 1, choice);
    proto::field_msg(&mut out, 12, &choice_msg);
    let mut cache = Vec::new();
    proto::field_varint(&mut cache, 1, CACHE_EPHEMERAL);
    proto::field_msg(&mut out, 13, &cache);
    proto::field_str(&mut out, 16, &cascade_id);
    proto::field_varint(&mut out, 20, PLANNER_MODE_DEFAULT);
    proto::field_str(&mut out, 21, &req.model);
    proto::field_str(&mut out, 22, &Uuid::new_v4().to_string());
    out
}

#[derive(Debug, Default)]
pub struct ToolCallDelta {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Default)]
pub struct ChatDelta {
    pub text: String,
    pub thinking: String,
    pub signature: String,
    pub signature_type: String,
    pub redacted: bool,
    pub tool_calls: Vec<ToolCallDelta>,
    pub usage: Option<Usage>,
    pub stop_reason: u64,
}

fn clamp_u32(value: Value<'_>) -> u32 {
    u32::try_from(as_u64(value).unwrap_or(0)).unwrap_or(u32::MAX)
}

fn usage_from(bytes: &[u8]) -> Usage {
    let mut usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };
    let mut reader = Reader::new(bytes);
    while let Some((field, value)) = reader.next() {
        match field {
            2 => usage.input_tokens = clamp_u32(value),
            3 => usage.output_tokens = clamp_u32(value),
            4 => usage.cache_write_tokens = clamp_u32(value),
            5 => usage.cache_read_tokens = clamp_u32(value),
            _ => {}
        }
    }
    usage
}

fn tool_call_from(bytes: &[u8]) -> ToolCallDelta {
    let mut call = ToolCallDelta::default();
    let mut reader = Reader::new(bytes);
    while let Some((field, value)) = reader.next() {
        match field {
            1 => as_str(value).unwrap_or("").clone_into(&mut call.id),
            2 => as_str(value).unwrap_or("").clone_into(&mut call.name),
            3 => as_str(value).unwrap_or("").clone_into(&mut call.arguments),
            _ => {}
        }
    }
    call
}

pub fn chat_response(payload: &[u8]) -> ChatDelta {
    let mut delta = ChatDelta::default();
    let mut reader = Reader::new(payload);
    while let Some((field, value)) = reader.next() {
        match field {
            3 => as_str(value).unwrap_or("").clone_into(&mut delta.text),
            5 => delta.stop_reason = as_u64(value).unwrap_or(0),
            6 => {
                if let Value::Bytes(call) = value {
                    delta.tool_calls.push(tool_call_from(call));
                }
            }
            7 => {
                if let Value::Bytes(usage) = value {
                    delta.usage = Some(usage_from(usage));
                }
            }
            9 => as_str(value).unwrap_or("").clone_into(&mut delta.thinking),
            10 => as_str(value).unwrap_or("").clone_into(&mut delta.signature),
            11 => delta.redacted = as_u64(value) == Some(1),
            21 => as_str(value)
                .unwrap_or("")
                .clone_into(&mut delta.signature_type),
            _ => {}
        }
    }
    delta
}

#[cfg(test)]
mod tests {
    use goat_provider::{Message, MessageRole, Request, ToolChoice};

    use super::*;
    use crate::proto::{Reader, Value, as_str, as_u64};

    fn request() -> Request {
        Request {
            model: "swe-1-7".into(),
            messages: vec![
                Message::text(MessageRole::System, "system note"),
                Message::text(MessageRole::User, "hello"),
            ],
            tools: vec![],
            effort: None,
            tool_choice: ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            system: Some("top system".into()),
        }
    }

    #[test]
    fn chat_request_has_expected_fields() {
        let bytes = chat_request(&request(), "key", "jwt", "session-1");
        let mut fields = Vec::new();
        let mut reader = Reader::new(&bytes);
        while let Some((field, value)) = reader.next() {
            fields.push(field);
            match field {
                2 => assert_eq!(as_str(value), Some("top system\n\nsystem note")),
                7 => assert_eq!(as_u64(value), Some(REQUEST_TYPE_CASCADE)),
                11 => assert_eq!(as_u64(value), Some(1)),
                16 => assert_eq!(as_str(value), Some("session-1")),
                20 => assert_eq!(as_u64(value), Some(PLANNER_MODE_DEFAULT)),
                21 => assert_eq!(as_str(value), Some("swe-1-7")),
                _ => {}
            }
        }
        assert!(fields.contains(&1));
        assert!(fields.contains(&3));
        assert!(fields.contains(&8));
        assert!(fields.contains(&13));
        assert!(fields.contains(&22));
    }

    #[test]
    fn tool_result_maps_to_tool_source() {
        let mut req = request();
        req.messages = vec![
            Message {
                role: MessageRole::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        text: "ponder".into(),
                        signature: "sig".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "call-1".into(),
                        name: "Read".into(),
                        input: serde_json::json!({"path": "x"}),
                    },
                ],
            },
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-1".into(),
                    content: vec![ContentBlock::Text {
                        text: "done".into(),
                    }],
                    is_error: true,
                }],
            },
        ];
        let bytes = chat_request(&req, "key", "jwt", "session-1");
        let mut prompts = Vec::new();
        let mut reader = Reader::new(&bytes);
        while let Some((field, value)) = reader.next() {
            if field == 3
                && let Value::Bytes(prompt) = value
            {
                prompts.push(prompt.to_vec());
            }
        }
        assert_eq!(prompts.len(), 2);
        let mut saw_tool = false;
        for prompt in prompts {
            let mut source = 0;
            let mut is_error = false;
            let mut tool_call_id = String::new();
            let mut inner = Reader::new(&prompt);
            while let Some((f, v)) = inner.next() {
                match f {
                    2 => source = as_u64(v).unwrap_or(0),
                    7 => tool_call_id = as_str(v).unwrap_or("").to_owned(),
                    9 => is_error = as_u64(v) == Some(1),
                    _ => {}
                }
            }
            if source == SOURCE_TOOL {
                saw_tool = true;
                assert_eq!(tool_call_id, "call-1");
                assert!(is_error);
            }
        }
        assert!(saw_tool);
    }

    #[test]
    fn chat_response_decodes_deltas() {
        let mut call = Vec::new();
        proto::field_str(&mut call, 1, "id-1");
        proto::field_str(&mut call, 2, "Read");
        proto::field_str(&mut call, 3, "{\"path\":");
        let mut usage = Vec::new();
        proto::field_varint(&mut usage, 2, 100);
        proto::field_varint(&mut usage, 3, 7);
        let mut msg = Vec::new();
        proto::field_str(&mut msg, 3, "chunk");
        proto::field_varint(&mut msg, 5, 3);
        proto::field_msg(&mut msg, 6, &call);
        proto::field_msg(&mut msg, 7, &usage);
        proto::field_str(&mut msg, 9, "thinking-bit");
        proto::field_str(&mut msg, 10, "sig-xyz");
        let delta = chat_response(&msg);
        assert_eq!(delta.text, "chunk");
        assert_eq!(delta.thinking, "thinking-bit");
        assert_eq!(delta.signature, "sig-xyz");
        assert_eq!(delta.stop_reason, 3);
        assert_eq!(delta.tool_calls.len(), 1);
        assert_eq!(delta.tool_calls[0].name, "Read");
        let usage = delta.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 7);
    }
}

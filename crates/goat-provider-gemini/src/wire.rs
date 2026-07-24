use std::collections::HashMap;

use goat_provider::{
    ContentBlock, Effort, MessageRole, Request, StreamChunk, ToolDefinition, Usage,
};
use serde_json::{Value, json};

pub fn gemini_efforts(model: &str) -> Vec<Effort> {
    let id = model.to_ascii_lowercase();
    if id.contains("3.1-pro") || id.contains("2.5-pro") {
        vec![Effort::Low, Effort::Medium, Effort::High, Effort::Max]
    } else {
        vec![Effort::Off, Effort::Low, Effort::Medium, Effort::High]
    }
}

fn is_25(model: &str) -> bool {
    model.to_ascii_lowercase().contains("2.5")
}

fn is_25_pro(model: &str) -> bool {
    let id = model.to_ascii_lowercase();
    id.contains("2.5") && id.contains("pro")
}

fn merge_generation_config(
    base: Option<Value>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Option<Value> {
    if temperature.is_none() && max_tokens.is_none() {
        return base;
    }
    let mut obj = match base {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    if let Some(temperature) = temperature {
        obj.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(max_tokens) = max_tokens {
        obj.insert("maxOutputTokens".to_owned(), json!(max_tokens));
    }
    Some(Value::Object(obj))
}

pub fn generation_config(model: &str, effort: Option<Effort>) -> Option<Value> {
    let effort = effort?;
    if is_25(model) {
        let budget: Option<i64> = if is_25_pro(model) {
            match effort {
                Effort::Off => return None,
                Effort::Low => Some(512),
                Effort::Medium => Some(4096),
                Effort::High => Some(16384),
                Effort::Xhigh | Effort::Max => Some(-1),
            }
        } else {
            match effort {
                Effort::Off => Some(0),
                Effort::Low => Some(512),
                Effort::Medium => Some(4096),
                Effort::High => Some(16384),
                Effort::Xhigh | Effort::Max => Some(-1),
            }
        };
        let budget = budget?;
        Some(json!({
            "thinkingConfig": {
                "thinkingBudget": budget,
                "includeThoughts": true,
            }
        }))
    } else {
        let level = match effort {
            Effort::Off => "MINIMAL",
            Effort::Low => "LOW",
            Effort::Medium => "MEDIUM",
            Effort::High | Effort::Xhigh | Effort::Max => "HIGH",
        };
        Some(json!({ "thinkingConfig": { "thinkingLevel": level } }))
    }
}

fn sanitize_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let cleaned: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "$schema" | "additionalProperties" | "$defs" | "definitions"
                    )
                })
                .map(|(k, v)| (k.clone(), sanitize_schema(v)))
                .collect();
            Value::Object(cleaned)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sanitize_schema).collect()),
        other => other.clone(),
    }
}

fn tool_declarations(tools: &[ToolDefinition]) -> Value {
    let decls: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parameters": sanitize_schema(&t.input_schema),
            })
        })
        .collect();
    json!([{ "functionDeclarations": decls }])
}

fn is_synthetic_id(id: &str) -> bool {
    id.starts_with("goat-")
}

fn content_block_to_part(
    block: &ContentBlock,
    id_to_name: &HashMap<String, String>,
    synthetic_counter: &mut u32,
) -> (Option<String>, Value) {
    match block {
        ContentBlock::Text { text } => (None, json!({ "text": text })),
        ContentBlock::Thinking { text, signature } => {
            if signature.is_empty() {
                (None, json!({ "text": text, "thought": true }))
            } else {
                (
                    None,
                    json!({ "text": text, "thought": true, "thoughtSignature": signature }),
                )
            }
        }
        ContentBlock::RedactedThinking { data } => {
            (None, json!({ "thought": true, "thoughtSignature": data }))
        }
        ContentBlock::ToolUse { id, name, input } => {
            let args = if input.is_object() {
                input.clone()
            } else {
                json!({})
            };
            let fc = if is_synthetic_id(id) {
                *synthetic_counter += 1;
                json!({ "functionCall": { "name": name, "args": args } })
            } else {
                json!({ "functionCall": { "name": name, "args": args, "id": id } })
            };
            (None, fc)
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let func_name = id_to_name
                .get(tool_use_id.as_str())
                .cloned()
                .unwrap_or_else(|| tool_use_id.clone());
            let output_text = ContentBlock::tool_result_text(content);
            let response_body = if *is_error {
                json!({ "error": output_text })
            } else {
                json!({ "output": output_text })
            };
            let fr = if is_synthetic_id(tool_use_id) {
                json!({ "functionResponse": { "name": func_name, "response": response_body } })
            } else {
                json!({
                    "functionResponse": {
                        "name": func_name,
                        "id": tool_use_id,
                        "response": response_body,
                    }
                })
            };
            (Some("user".to_owned()), fr)
        }
        ContentBlock::Image { media_type, data } => (
            None,
            json!({ "inlineData": { "mimeType": media_type, "data": data } }),
        ),
    }
}

pub struct InnerRequest {
    pub contents: Vec<Value>,
    pub system_instruction: Option<Value>,
    pub tools: Option<Value>,
    pub tool_config: Option<Value>,
    pub generation_config: Option<Value>,
}

pub fn build_request(req: &Request) -> InnerRequest {
    let mut id_to_name: HashMap<String, String> = HashMap::new();
    for msg in &req.messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                id_to_name.insert(id.clone(), name.clone());
            }
        }
    }

    let mut system_parts: Vec<Value> = Vec::new();
    if let Some(system) = req.system.as_deref()
        && !system.is_empty()
    {
        system_parts.push(json!({ "text": system }));
    }
    let mut contents: Vec<Value> = Vec::new();
    let mut synthetic_counter: u32 = 0;

    for msg in &req.messages {
        match msg.role {
            MessageRole::System => {
                for block in &msg.content {
                    if let ContentBlock::Text { text } = block {
                        system_parts.push(json!({ "text": text }));
                    }
                }
            }
            MessageRole::User => {
                let mut parts: Vec<Value> = Vec::new();
                let mut pending_fr: Vec<Value> = Vec::new();
                for block in &msg.content {
                    let (override_role, part) =
                        content_block_to_part(block, &id_to_name, &mut synthetic_counter);
                    if override_role.is_some() {
                        pending_fr.push(part);
                    } else {
                        parts.push(part);
                    }
                }
                if !parts.is_empty() {
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
                if !pending_fr.is_empty() {
                    contents.push(json!({ "role": "user", "parts": pending_fr }));
                }
            }
            MessageRole::Assistant => {
                let parts: Vec<Value> = msg
                    .content
                    .iter()
                    .map(|b| content_block_to_part(b, &id_to_name, &mut synthetic_counter).1)
                    .collect();
                if !parts.is_empty() {
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
            }
        }
    }

    let contents = coalesce_user_text_contents(contents);

    let system_instruction = if system_parts.is_empty() {
        None
    } else {
        Some(json!({ "parts": system_parts }))
    };

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(tool_declarations(&req.tools))
    };

    let tool_config = (tools.is_some()
        && matches!(req.tool_choice, goat_provider::ToolChoice::None))
    .then(|| json!({ "functionCallingConfig": { "mode": "NONE" } }));

    let gen_cfg = merge_generation_config(
        generation_config(&req.model, req.effort),
        req.temperature,
        req.max_tokens,
    );

    InnerRequest {
        contents,
        system_instruction,
        tools,
        tool_config,
        generation_config: gen_cfg,
    }
}

fn is_plain_user_content(content: &Value) -> bool {
    content.get("role").and_then(Value::as_str) == Some("user")
        && content
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts
                    .iter()
                    .all(|part| part.get("functionResponse").is_none())
            })
}

fn coalesce_user_text_contents(contents: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for mut content in contents {
        if let Some(last) = out.last_mut()
            && is_plain_user_content(last)
            && is_plain_user_content(&content)
            && let (Some(Value::Array(dst)), Some(Value::Array(src))) =
                (last.get_mut("parts"), content.get_mut("parts"))
        {
            dst.append(src);
            continue;
        }
        out.push(content);
    }
    out
}

pub fn inner_request_to_value(inner: InnerRequest) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("contents".to_owned(), Value::Array(inner.contents));
    if let Some(si) = inner.system_instruction {
        obj.insert("systemInstruction".to_owned(), si);
    }
    if let Some(tools) = inner.tools {
        obj.insert("tools".to_owned(), tools);
    }
    if let Some(tool_config) = inner.tool_config {
        obj.insert("toolConfig".to_owned(), tool_config);
    }
    if let Some(gc) = inner.generation_config {
        obj.insert("generationConfig".to_owned(), gc);
    }
    Value::Object(obj)
}

static SYNTHETIC_TOOL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn parse_chunk(value: &Value, oauth: bool) -> Vec<StreamChunk> {
    let payload = if oauth {
        value.get("response").unwrap_or(value)
    } else {
        value
    };

    let mut events = Vec::new();
    let Some(parts) = payload
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
    else {
        return events;
    };

    for part in parts {
        if let Some(fc) = part.get("functionCall") {
            let name = fc
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let id = fc.get("id").and_then(Value::as_str).map_or_else(
                || {
                    let n = SYNTHETIC_TOOL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    format!("goat-{n}")
                },
                str::to_owned,
            );
            let input = fc
                .get("args")
                .map_or_else(|| "{}".to_owned(), Value::to_string);
            events.push(StreamChunk::ToolCall { id, name, input });
            continue;
        }

        let is_thought = part
            .get("thought")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let text = part.get("text").and_then(Value::as_str).unwrap_or("");

        if is_thought {
            if !text.is_empty() {
                events.push(StreamChunk::ThinkingDelta {
                    text: text.to_owned(),
                });
            }
            if let Some(sig) = part.get("thoughtSignature").and_then(Value::as_str)
                && !sig.is_empty()
            {
                events.push(StreamChunk::ThinkingSignature {
                    signature: sig.to_owned(),
                });
            }
        } else if !text.is_empty() {
            events.push(StreamChunk::TextDelta {
                text: text.to_owned(),
            });
        }
    }

    events
}

pub fn extract_finish_reason(value: &Value, oauth: bool) -> Option<&str> {
    let payload = if oauth {
        value.get("response").unwrap_or(value)
    } else {
        value
    };
    payload
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finishReason"))
        .and_then(Value::as_str)
}

pub fn parse_usage(value: &Value, oauth: bool) -> Option<Usage> {
    let payload = if oauth {
        value.get("response").unwrap_or(value)
    } else {
        value
    };
    let meta = payload.get("usageMetadata")?;
    let count = |key: &str| -> u32 {
        meta.get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    Some(Usage {
        input_tokens: count("promptTokenCount"),
        output_tokens: count("candidatesTokenCount") + count("thoughtsTokenCount"),
        cache_read_tokens: count("cachedContentTokenCount"),
        cache_write_tokens: 0,
    })
}

#[cfg(test)]
mod tests {
    use goat_provider::{ContentBlock, Effort, Message, MessageRole, Request, ToolDefinition};
    use serde_json::json;

    use super::{
        build_request, gemini_efforts, generation_config, inner_request_to_value, parse_chunk,
        parse_usage,
    };

    fn make_request(messages: Vec<Message>) -> Request {
        Request {
            model: "gemini-2.5-flash".to_owned(),
            messages,
            tool_choice: goat_provider::ToolChoice::Auto,
            tools: vec![],
            effort: None,
            temperature: None,
            max_tokens: None,
            system: None,
        }
    }

    #[test]
    fn text_message_maps_to_user_part() {
        let req = make_request(vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_owned(),
            }],
        }]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        assert_eq!(v["contents"][0]["role"], "user");
        assert_eq!(v["contents"][0]["parts"][0]["text"], "hello");
    }

    #[test]
    fn system_message_maps_to_system_instruction() {
        let req = make_request(vec![Message {
            role: MessageRole::System,
            content: vec![ContentBlock::Text {
                text: "be helpful".to_owned(),
            }],
        }]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        assert!(v.get("systemInstruction").is_some());
        assert_eq!(v["systemInstruction"]["parts"][0]["text"], "be helpful");
        assert!(v["contents"].as_array().is_none_or(Vec::is_empty));
    }

    #[test]
    fn assistant_message_maps_to_model_role() {
        let req = make_request(vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text {
                text: "hi".to_owned(),
            }],
        }]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        assert_eq!(v["contents"][0]["role"], "model");
    }

    #[test]
    fn thinking_block_with_signature() {
        let req = make_request(vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Thinking {
                text: "ponder".to_owned(),
                signature: "sig123".to_owned(),
            }],
        }]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        let part = &v["contents"][0]["parts"][0];
        assert_eq!(part["thought"], true);
        assert_eq!(part["text"], "ponder");
        assert_eq!(part["thoughtSignature"], "sig123");
    }

    #[test]
    fn thinking_block_empty_signature_omits_field() {
        let req = make_request(vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Thinking {
                text: "think".to_owned(),
                signature: String::new(),
            }],
        }]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        let part = &v["contents"][0]["parts"][0];
        assert!(part.get("thoughtSignature").is_none());
    }

    #[test]
    fn tool_use_real_id_included() {
        let req = make_request(vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "real-id-123".to_owned(),
                name: "my_tool".to_owned(),
                input: json!({ "x": 1 }),
            }],
        }]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        let fc = &v["contents"][0]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "my_tool");
        assert_eq!(fc["id"], "real-id-123");
    }

    #[test]
    fn tool_use_synthetic_id_omitted() {
        let req = make_request(vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "goat-1".to_owned(),
                name: "my_tool".to_owned(),
                input: json!({}),
            }],
        }]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        let fc = &v["contents"][0]["parts"][0]["functionCall"];
        assert!(fc.get("id").is_none());
        assert_eq!(fc["name"], "my_tool");
    }

    #[test]
    fn consecutive_user_text_contents_merge() {
        let req = make_request(vec![
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "first".to_owned(),
                }],
            },
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "second".to_owned(),
                }],
            },
        ]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        assert_eq!(v["contents"].as_array().unwrap().len(), 1);
        assert_eq!(v["contents"][0]["role"], "user");
        assert_eq!(v["contents"][0]["parts"][0]["text"], "first");
        assert_eq!(v["contents"][0]["parts"][1]["text"], "second");
    }

    #[test]
    fn function_response_content_does_not_merge() {
        let req = make_request(vec![
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "run it".to_owned(),
                }],
            },
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::text_result(
                    "real-id-1".to_owned(),
                    "done",
                    false,
                )],
            },
        ]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        assert_eq!(v["contents"].as_array().unwrap().len(), 2);
        assert!(
            v["contents"][1]["parts"][0]
                .get("functionResponse")
                .is_some()
        );
    }

    #[test]
    fn tool_result_uses_id_to_name_map() {
        let req = make_request(vec![
            Message {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "real-id-1".to_owned(),
                    name: "read_file".to_owned(),
                    input: json!({}),
                }],
            },
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::text_result(
                    "real-id-1".to_owned(),
                    "file content",
                    false,
                )],
            },
        ]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        let fr = &v["contents"][1]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "read_file");
        assert_eq!(fr["id"], "real-id-1");
        assert_eq!(fr["response"]["output"], "file content");
    }

    #[test]
    fn tool_result_synthetic_id_omits_id_field() {
        let req = make_request(vec![
            Message {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "goat-1".to_owned(),
                    name: "write_file".to_owned(),
                    input: json!({}),
                }],
            },
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::text_result("goat-1".to_owned(), "ok", false)],
            },
        ]);
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        let fr = &v["contents"][1]["parts"][0]["functionResponse"];
        assert!(fr.get("id").is_none());
        assert_eq!(fr["name"], "write_file");
    }

    #[test]
    fn generation_config_25_flash_off_returns_zero_budget() {
        let cfg = generation_config("gemini-2.5-flash", Some(Effort::Off)).unwrap();
        assert_eq!(cfg["thinkingConfig"]["thinkingBudget"], 0);
        assert_eq!(cfg["thinkingConfig"]["includeThoughts"], true);
    }

    #[test]
    fn generation_config_25_pro_off_returns_none() {
        let cfg = generation_config("gemini-2.5-pro", Some(Effort::Off));
        assert!(cfg.is_none());
    }

    #[test]
    fn generation_config_25_flash_medium() {
        let cfg = generation_config("gemini-2.5-flash", Some(Effort::Medium)).unwrap();
        assert_eq!(cfg["thinkingConfig"]["thinkingBudget"], 4096);
    }

    #[test]
    fn generation_config_25_max_dynamic() {
        let cfg = generation_config("gemini-2.5-flash", Some(Effort::Max)).unwrap();
        assert_eq!(cfg["thinkingConfig"]["thinkingBudget"], -1);
    }

    #[test]
    fn generation_config_3x_flash_uses_level() {
        let cfg = generation_config("gemini-3.5-flash", Some(Effort::Medium)).unwrap();
        assert_eq!(cfg["thinkingConfig"]["thinkingLevel"], "MEDIUM");
        assert!(cfg["thinkingConfig"].get("thinkingBudget").is_none());
    }

    #[test]
    fn generation_config_3x_off_maps_to_minimal() {
        let cfg = generation_config("gemini-3.5-flash", Some(Effort::Off)).unwrap();
        assert_eq!(cfg["thinkingConfig"]["thinkingLevel"], "MINIMAL");
    }

    #[test]
    fn generation_config_none_effort_returns_none() {
        assert!(generation_config("gemini-2.5-flash", None).is_none());
    }

    #[test]
    fn gemini_efforts_pro_no_off() {
        let e = gemini_efforts("gemini-2.5-pro");
        assert!(!e.contains(&Effort::Off));
        assert!(e.contains(&Effort::High));
    }

    #[test]
    fn gemini_efforts_flash_has_off() {
        let e = gemini_efforts("gemini-2.5-flash");
        assert!(e.contains(&Effort::Off));
    }

    #[test]
    fn parse_chunk_text_delta() {
        let chunk = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "hello" }] }
            }]
        });
        let events = parse_chunk(&chunk, false);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], goat_provider::StreamChunk::TextDelta { text } if text == "hello")
        );
    }

    #[test]
    fn parse_chunk_thought() {
        let chunk = json!({
            "candidates": [{
                "content": { "parts": [{ "thought": true, "text": "thinking..." }] }
            }]
        });
        let events = parse_chunk(&chunk, false);
        assert!(matches!(
            &events[0],
            goat_provider::StreamChunk::ThinkingDelta { .. }
        ));
    }

    #[test]
    fn parse_chunk_function_call_no_id_uses_synthetic() {
        let chunk = json!({
            "candidates": [{
                "content": { "parts": [{ "functionCall": { "name": "foo", "args": {} } }] }
            }]
        });
        let events = parse_chunk(&chunk, false);
        assert!(
            matches!(&events[0], goat_provider::StreamChunk::ToolCall { id, .. } if id.starts_with("goat-"))
        );
    }

    #[test]
    fn parse_chunk_oauth_unwraps_response() {
        let chunk = json!({
            "response": {
                "candidates": [{
                    "content": { "parts": [{ "text": "wrapped" }] }
                }]
            }
        });
        let events = parse_chunk(&chunk, true);
        assert!(
            matches!(&events[0], goat_provider::StreamChunk::TextDelta { text } if text == "wrapped")
        );
    }

    #[test]
    fn tool_request_serialized() {
        let req = Request {
            model: "gemini-2.5-flash".to_owned(),
            messages: vec![],
            tool_choice: goat_provider::ToolChoice::Auto,
            tools: vec![ToolDefinition {
                name: "fn1".to_owned(),
                description: "does fn1".to_owned(),
                input_schema: json!({ "type": "object", "$schema": "ignored" }),
            }],
            effort: None,
            temperature: None,
            max_tokens: None,
            system: None,
        };
        let inner = build_request(&req);
        let v = inner_request_to_value(inner);
        let decl = &v["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "fn1");
        assert!(decl["parameters"].get("$schema").is_none());
    }

    #[test]
    fn parse_usage_sums_candidates_and_thoughts() {
        let chunk = json!({
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 40,
                "thoughtsTokenCount": 25,
                "cachedContentTokenCount": 10
            }
        });
        let usage = parse_usage(&chunk, false).expect("usage");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 65);
        assert_eq!(usage.cache_read_tokens, 10);
        assert_eq!(usage.cache_write_tokens, 0);
    }

    #[test]
    fn parse_usage_oauth_unwraps_response() {
        let chunk = json!({
            "response": { "usageMetadata": { "promptTokenCount": 7 } }
        });
        let usage = parse_usage(&chunk, true).expect("usage");
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 0);
    }

    #[test]
    fn parse_usage_absent_returns_none() {
        let chunk = json!({ "candidates": [] });
        assert!(parse_usage(&chunk, false).is_none());
    }
}

use goat_provider::ToolDefinition;

const NS: [&str; 2] = ["", "antml:"];

pub(crate) fn recover(
    provider: &str,
    raw: &str,
    tool_defs: &[ToolDefinition],
) -> (String, Vec<(String, String)>) {
    let (mut clean, mut calls) = parse_invoke_blocks(raw);
    clean = strip_wrapper_tags(&clean);
    if text_leaky_provider(provider) {
        let (next, hermes) = parse_json_tag_blocks(&clean, "tool_call");
        clean = next;
        calls.extend(hermes);
        let (next, mistral) = parse_mistral_blocks(&clean);
        clean = next;
        calls.extend(mistral);
    }
    let known = calls
        .into_iter()
        .filter(|(name, _)| tool_defs.iter().any(|def| def.name == *name))
        .collect();
    (clean, known)
}

pub(crate) fn input_equivalent(a: &str, b: &str) -> bool {
    match (
        serde_json::from_str::<serde_json::Value>(a),
        serde_json::from_str::<serde_json::Value>(b),
    ) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

const LEAKY_TEXT_PROVIDERS: [&str; 3] = ["ollama", "lmstudio", "llama-cpp"];

fn text_leaky_provider(provider: &str) -> bool {
    LEAKY_TEXT_PROVIDERS.contains(&provider)
}

fn strip_wrapper_tags(text: &str) -> String {
    let mut out = text.to_owned();
    for ns in NS {
        out = out.replace(&format!("<{ns}function_calls>"), "");
        out = out.replace(&format!("</{ns}function_calls>"), "");
    }
    out
}

fn parse_invoke_blocks(raw: &str) -> (String, Vec<(String, String)>) {
    let mut clean = String::new();
    let mut calls = Vec::new();
    let mut cursor = 0;
    loop {
        let Some((start, after_name)) = find_open(raw, cursor, "invoke") else {
            clean.push_str(&raw[cursor..]);
            break;
        };
        clean.push_str(&raw[cursor..start]);
        let Some(open_end) = raw[after_name..].find('>').map(|rel| after_name + rel + 1) else {
            break;
        };
        let name = attr_value(&raw[after_name..open_end], "name").unwrap_or_default();
        let Some((close_start, close_end)) = find_close(raw, open_end, "invoke") else {
            break;
        };
        let params = parse_params(&raw[open_end..close_start]);
        if !name.is_empty() {
            calls.push((name, params_to_json(&params)));
        }
        cursor = close_end;
    }
    (clean, calls)
}

fn parse_params(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some((_, after_name)) = find_open(body, cursor, "parameter") {
        let Some(open_end) = body[after_name..].find('>').map(|rel| after_name + rel + 1) else {
            break;
        };
        let name = attr_value(&body[after_name..open_end], "name").unwrap_or_default();
        let Some((close_start, close_end)) = find_close(body, open_end, "parameter") else {
            break;
        };
        if !name.is_empty() {
            out.push((name, body[open_end..close_start].to_owned()));
        }
        cursor = close_end;
    }
    out
}

fn parse_json_tag_blocks(raw: &str, tag: &str) -> (String, Vec<(String, String)>) {
    let mut clean = String::new();
    let mut calls = Vec::new();
    let mut cursor = 0;
    loop {
        let Some((start, after_name)) = find_open(raw, cursor, tag) else {
            clean.push_str(&raw[cursor..]);
            break;
        };
        clean.push_str(&raw[cursor..start]);
        let Some(open_end) = raw[after_name..].find('>').map(|rel| after_name + rel + 1) else {
            break;
        };
        let Some((close_start, close_end)) = find_close(raw, open_end, tag) else {
            break;
        };
        if let Some(call) =
            serde_json::from_str::<serde_json::Value>(raw[open_end..close_start].trim())
                .ok()
                .as_ref()
                .and_then(value_call)
        {
            calls.push(call);
        }
        cursor = close_end;
    }
    (clean, calls)
}

fn parse_mistral_blocks(raw: &str) -> (String, Vec<(String, String)>) {
    const MARK: &str = "[TOOL_CALLS]";
    let mut clean = String::new();
    let mut calls = Vec::new();
    let mut cursor = 0;
    loop {
        let Some(rel) = raw[cursor..].find(MARK) else {
            clean.push_str(&raw[cursor..]);
            break;
        };
        let mark_start = cursor + rel;
        clean.push_str(&raw[cursor..mark_start]);
        let after_mark = mark_start + MARK.len();
        let json_start = raw.len() - raw[after_mark..].trim_start().len();
        let mut stream =
            serde_json::Deserializer::from_str(&raw[json_start..]).into_iter::<serde_json::Value>();
        if let Some(Ok(value)) = stream.next() {
            match value.as_array() {
                Some(items) => calls.extend(items.iter().filter_map(value_call)),
                None => calls.extend(value_call(&value)),
            }
            cursor = json_start + stream.byte_offset();
        } else {
            cursor = after_mark;
        }
    }
    (clean, calls)
}

fn value_call(value: &serde_json::Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    let name = object.get("name")?.as_str()?.to_owned();
    let input = match object.get("arguments").or_else(|| object.get("parameters")) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "{}".to_owned(),
    };
    Some((name, input))
}

fn find_open(hay: &str, from: usize, name: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for ns in NS {
        let pat = format!("<{ns}{name}");
        if let Some(rel) = hay[from..].find(&pat) {
            let start = from + rel;
            let after = start + pat.len();
            let boundary = hay[after..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == '>' || c == '/');
            if boundary && best.is_none_or(|(b, _)| start < b) {
                best = Some((start, after));
            }
        }
    }
    best
}

fn find_close(hay: &str, from: usize, name: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for ns in NS {
        let pat = format!("</{ns}{name}>");
        if let Some(rel) = hay[from..].find(&pat) {
            let start = from + rel;
            if best.is_none_or(|(b, _)| start < b) {
                best = Some((start, start + pat.len()));
            }
        }
    }
    best
}

fn attr_value(attrs: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=");
    let rest = &attrs[attrs.find(&pat)? + pat.len()..];
    let quote = rest.chars().find(|c| *c == '"' || *c == '\'')?;
    let after = &rest[rest.find(quote)? + 1..];
    Some(after[..after.find(quote)?].to_owned())
}

fn params_to_json(params: &[(String, String)]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = params
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::{input_equivalent, recover};
    use goat_provider::ToolDefinition;

    fn defs(names: &[&str]) -> Vec<ToolDefinition> {
        names
            .iter()
            .map(|name| ToolDefinition {
                name: (*name).to_owned(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            })
            .collect()
    }

    fn value(input: &str) -> serde_json::Value {
        serde_json::from_str(input).expect("valid json")
    }

    fn open(ns: &str, tag: &str, attr: &str) -> String {
        format!("<{ns}{tag} name=\"{attr}\">")
    }

    fn close(ns: &str, tag: &str) -> String {
        format!("</{ns}{tag}>")
    }

    fn block(ns: &str, name: &str, params: &[(&str, &str)]) -> String {
        let mut out = open(ns, "invoke", name);
        for (k, v) in params {
            out.push('\n');
            out.push_str(&open(ns, "parameter", k));
            out.push_str(v);
            out.push_str(&close(ns, "parameter"));
        }
        out.push('\n');
        out.push_str(&close(ns, "invoke"));
        out
    }

    #[test]
    fn parses_real_leaked_sample() {
        let raw = format!(
            "이제 product-card의 status를 반영합니다.\n\ncount\n{}",
            block(
                "",
                "Edit",
                &[
                    ("new_string", "  const isSoldOut = a;"),
                    ("old_string", "  const isSoldOut = b;"),
                    ("path", "/abs/product-card.tsx"),
                ],
            )
        );
        let (clean, calls) = recover("anthropic", &raw, &defs(&["Edit"]));
        assert!(!clean.contains("invoke"), "clean still leaks: {clean}");
        assert!(clean.contains("반영합니다."));
        assert!(clean.contains("count"));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Edit");
        assert_eq!(
            value(&calls[0].1),
            value(
                r#"{"new_string":"  const isSoldOut = a;","old_string":"  const isSoldOut = b;","path":"/abs/product-card.tsx"}"#
            )
        );
    }

    #[test]
    fn parses_namespaced_variant() {
        let raw = format!("prose {}", block("antml:", "Read", &[("path", "/a.txt")]));
        let (clean, calls) = recover("anthropic", &raw, &defs(&["Read"]));
        assert!(!clean.contains("invoke"), "clean still leaks: {clean}");
        assert!(clean.contains("prose"));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Read");
        assert_eq!(value(&calls[0].1), value(r#"{"path":"/a.txt"}"#));
    }

    #[test]
    fn parses_multiple_invokes() {
        let raw = format!(
            "{}\nthen\n{}",
            block("", "Read", &[("path", "/a")]),
            block("", "Read", &[("path", "/b")]),
        );
        let (clean, calls) = recover("anthropic", &raw, &defs(&["Read"]));
        assert!(!clean.contains("invoke"));
        assert!(clean.contains("then"));
        assert_eq!(calls.len(), 2);
        assert_eq!(value(&calls[0].1), value(r#"{"path":"/a"}"#));
        assert_eq!(value(&calls[1].1), value(r#"{"path":"/b"}"#));
    }

    #[test]
    fn param_value_with_angle_brackets_preserved() {
        let raw = block("", "Edit", &[("new_string", "if (a < b && c > d) {}")]);
        let (clean, calls) = recover("anthropic", &raw, &defs(&["Edit"]));
        assert!(!clean.contains("invoke"));
        assert_eq!(
            value(&calls[0].1),
            value(r#"{"new_string":"if (a < b && c > d) {}"}"#)
        );
    }

    #[test]
    fn truncated_invoke_is_stripped_not_recovered() {
        let raw = format!("prose\n{} <no close", open("", "invoke", "Edit"));
        let (clean, calls) = recover("anthropic", &raw, &defs(&["Edit"]));
        assert!(calls.is_empty());
        assert!(
            !clean.contains("invoke"),
            "truncated tail must be stripped: {clean}"
        );
        assert!(clean.contains("prose"));
    }

    #[test]
    fn unknown_tool_is_stripped_but_not_recovered() {
        let raw = format!("x {}", block("", "Bash", &[("cmd", "rm -rf /")]));
        let (clean, calls) = recover("anthropic", &raw, &defs(&["Edit", "Read"]));
        assert!(calls.is_empty(), "unknown tool must not be dispatched");
        assert!(
            !clean.contains("invoke"),
            "unknown span must still be stripped: {clean}"
        );
    }

    #[test]
    fn plain_prose_is_untouched() {
        let raw = "just talking about the invoke method and parameters, no tags.";
        let (clean, calls) = recover("anthropic", raw, &defs(&["Edit"]));
        assert_eq!(clean, raw);
        assert!(calls.is_empty());
    }

    #[test]
    fn recovers_hermes_tool_call_for_local() {
        let raw =
            "sure\n<tool_call>{\"name\": \"Read\", \"arguments\": {\"path\": \"/a\"}}</tool_call>";
        let (clean, calls) = recover("ollama", raw, &defs(&["Read"]));
        assert!(!clean.contains("tool_call"), "clean still leaks: {clean}");
        assert!(clean.contains("sure"));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Read");
        assert_eq!(value(&calls[0].1), value(r#"{"path":"/a"}"#));
    }

    #[test]
    fn recovers_mistral_tool_calls_for_local() {
        let raw = "[TOOL_CALLS][{\"name\": \"Read\", \"arguments\": {\"path\": \"/a\"}}]";
        let (clean, calls) = recover("lmstudio", raw, &defs(&["Read"]));
        assert!(!clean.contains("TOOL_CALLS"), "clean still leaks: {clean}");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Read");
        assert_eq!(value(&calls[0].1), value(r#"{"path":"/a"}"#));
    }

    #[test]
    fn hermes_dialect_not_applied_to_non_local_provider() {
        let raw = "docs: emit <tool_call>{\"name\":\"X\"}</tool_call> to call a tool.";
        let (clean, calls) = recover("anthropic", raw, &defs(&["X"]));
        assert_eq!(clean, raw, "non-local prose must be untouched");
        assert!(calls.is_empty());
    }

    #[test]
    fn strips_function_calls_wrapper() {
        let inner = block("", "Read", &[("path", "/a")]);
        let raw = format!("pre <{w}>{inner}</{w}> post", w = "function_calls");
        let (clean, calls) = recover("anthropic", &raw, &defs(&["Read"]));
        assert!(
            !clean.contains("function_calls"),
            "wrapper residue: {clean}"
        );
        assert!(!clean.contains("invoke"));
        assert!(clean.contains("pre") && clean.contains("post"));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Read");
    }

    #[test]
    fn input_equivalent_ignores_key_order() {
        assert!(input_equivalent(r#"{"a":1,"b":2}"#, r#"{"b":2,"a":1}"#));
        assert!(!input_equivalent(r#"{"a":1}"#, r#"{"a":2}"#));
    }
}

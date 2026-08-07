use goat_protocol::ToolDisplay;
use serde_json::Value;

pub fn raw(input: &str) -> ToolDisplay {
    ToolDisplay::primary(flatten(input))
}

pub fn flatten(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let head: String = s.chars().take(max_chars).collect();
        format!("{head}…")
    } else {
        s.to_owned()
    }
}

pub fn format_arg(s: &str) -> String {
    let needs =
        s.is_empty() || s.chars().any(char::is_whitespace) || s.contains('"') || s.contains('\'');
    if needs {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_owned()
    }
}

pub fn call_sig(name: &str, args: &[&str]) -> String {
    if args.is_empty() {
        format!("{name}()")
    } else {
        let inner: Vec<String> = args.iter().map(|a| format_arg(a)).collect();
        format!("{name}({})", inner.join(", "))
    }
}

pub fn generic(input: &str) -> ToolDisplay {
    generic_named("", input)
}

pub fn generic_named(tool_name: &str, input: &str) -> ToolDisplay {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(input) else {
        return raw(input);
    };
    let parts: Vec<String> = map.values().filter_map(scalar_text).take(3).collect();
    if parts.is_empty() {
        return raw(input);
    }
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    if tool_name.is_empty() {
        let mut iter = parts.into_iter();
        let primary = iter.next().unwrap_or_default();
        let rest: Vec<String> = iter.collect();
        if rest.is_empty() {
            ToolDisplay::primary(primary)
        } else {
            ToolDisplay::with_detail(primary, rest.join(", "))
        }
    } else {
        ToolDisplay::primary(call_sig(tool_name, &refs))
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(flatten(s)),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::ToolDisplay;

    use super::{call_sig, format_arg, generic, generic_named};

    #[test]
    fn generic_named_builds_call_sig() {
        let got = generic_named("Glob", r#"{"pattern":"**/symbols*"}"#);
        assert_eq!(got.primary, "Glob(**/symbols*)");
    }

    #[test]
    fn format_arg_quotes_spaces() {
        assert_eq!(format_arg("a b"), "\"a b\"");
        assert_eq!(format_arg("path/to"), "path/to");
    }

    #[test]
    fn call_sig_joins_args() {
        assert_eq!(call_sig("Read", &["a.txt"]), "Read(a.txt)");
        assert_eq!(
            call_sig("Read", &["/Users/jmo", "10", "20"]),
            "Read(/Users/jmo, 10, 20)"
        );
    }

    #[test]
    fn generic_displays_scalar_values() {
        let got = generic(r#"{"limit":5,"query":"rust tui"}"#);
        assert_eq!(got.primary, "5");
        assert_eq!(got.detail.as_deref(), Some("rust tui"));
    }

    #[test]
    fn generic_flattens_whitespace() {
        let got = generic(r#"{"command":"cargo \n  build"}"#);
        assert_eq!(got.primary, "cargo build");
    }

    #[test]
    fn non_json_passes_raw_through() {
        assert_eq!(generic("src/lib.rs"), ToolDisplay::primary("src/lib.rs"));
    }

    #[test]
    fn truncate_chars_appends_ellipsis_past_limit() {
        assert_eq!(super::truncate_chars("hello", 5), "hello");
        assert_eq!(super::truncate_chars("hello world", 5), "hello…");
        assert_eq!(super::truncate_chars("가나다라마", 3), "가나다…");
    }

    #[test]
    fn empty_object_passes_raw_through() {
        assert_eq!(generic("{}"), ToolDisplay::primary("{}"));
    }
}

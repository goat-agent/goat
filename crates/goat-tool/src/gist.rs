use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const ELLIPSIS: &str = "…";

pub fn transcript_sig(
    tool_name: &str,
    display_primary: &str,
    cwd: &str,
    width: u16,
    failed: bool,
) -> String {
    let full = normalize(tool_name, display_primary);
    let (name, args) = parse(tool_name, &full);
    let budget = arg_budget(width, failed, 64);
    let mut shortened = Vec::new();
    if let Some(first) = args.first() {
        let flattened = first.split_whitespace().collect::<Vec<_>>().join(" ");
        let relative = path_under_cwd(&flattened, cwd);
        let first = if relative != flattened || looks_like_path(&relative) {
            ellipsize_path_middle(&relative, budget)
        } else {
            clip_to_width(&flattened, budget)
        };
        shortened.push(first);
    }
    if args.get(1).is_some_and(|argument| argument != ".") {
        shortened.push(ELLIPSIS.to_owned());
    }
    format(&name, &shortened)
}

pub fn call_args(tool_name: &str, display_primary: &str) -> Vec<String> {
    let full = normalize(tool_name, display_primary);
    parse(tool_name, &full).1
}

fn normalize(tool_name: &str, display_primary: &str) -> String {
    let trimmed = display_primary.trim();
    if trimmed.is_empty() {
        return format!("{tool_name}()");
    }
    if let Some(open) = trimmed.find('(') {
        let head = trimmed[..open].trim();
        if head == tool_name {
            return trimmed.to_owned();
        }
        if let Some(args) = bare_args(trimmed) {
            return format_with_refs(tool_name, &args);
        }
    }
    format_with_refs(tool_name, &[trimmed.to_owned()])
}

fn parse(tool_name: &str, sig: &str) -> (String, Vec<String>) {
    let Some(open) = sig.find('(') else {
        return (tool_name.to_owned(), Vec::new());
    };
    let name = sig[..open].trim().to_owned();
    let tail = &sig[open..];
    if !tail.ends_with(')') || tail.len() < 2 {
        return (name, vec![tail.to_owned()]);
    }
    let inner = &tail[1..tail.len() - 1];
    if inner.is_empty() {
        return (name, Vec::new());
    }
    (
        name,
        split_top_level(inner)
            .iter()
            .map(|arg| unquote_arg(arg))
            .collect(),
    )
}

fn split_top_level(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if escaped {
            cur.push(c);
            escaped = false;
        } else if c == '\\' {
            cur.push(c);
            escaped = true;
        } else if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            cur.push(c);
            quote = Some(c);
        } else if c == ',' && chars.peek() == Some(&' ') {
            chars.next();
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

fn format(tool_name: &str, args: &[String]) -> String {
    if args.is_empty() {
        format!("{tool_name}()")
    } else {
        let parts: Vec<String> = args.iter().map(|arg| quote_arg_if_needed(arg)).collect();
        format!("{tool_name}({})", parts.join(", "))
    }
}

fn bare_args(sig: &str) -> Option<Vec<String>> {
    let open = sig.find('(')?;
    let tail = &sig[open..];
    if !tail.ends_with(')') || tail.len() < 2 {
        return None;
    }
    let inner = &tail[1..tail.len() - 1];
    if inner.is_empty() {
        return Some(Vec::new());
    }
    Some(split_top_level(inner))
}

fn format_with_refs(name: &str, args: &[String]) -> String {
    if args.is_empty() {
        format!("{name}()")
    } else {
        format!("{name}({})", args.join(", "))
    }
}

fn unquote_arg(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        trimmed.to_owned()
    }
}

fn quote_arg_if_needed(s: &str) -> String {
    let needs = s.is_empty()
        || s.chars().any(char::is_whitespace)
        || s.contains('"')
        || s.contains('\'')
        || s.contains(',');
    if needs {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_owned()
    }
}

fn looks_like_path(value: &str) -> bool {
    !value.chars().any(char::is_whitespace)
        && (value.starts_with('/') || value.starts_with("./") || value.matches('/').count() > 1)
}

fn path_under_cwd(raw: &str, cwd: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return raw.to_owned();
    }
    let cwd = cwd.trim_end_matches('/');
    let prefix = format!("{cwd}/");
    if let Some(rest) = raw.strip_prefix(&prefix) {
        return rest.to_owned();
    }
    if raw == cwd {
        return ".".to_owned();
    }
    raw.to_owned()
}

fn ellipsize_path_middle(path: &str, max: usize) -> String {
    if path.width() <= max {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return clip_to_width(path, max);
    }
    let file = parts.last().copied().unwrap_or("");
    if let Some(parent) = parts.get(parts.len().saturating_sub(2)) {
        let candidate = format!("…/{parent}/{file}");
        if candidate.width() <= max {
            return candidate;
        }
    }
    let tail = format!("…/{file}");
    if tail.width() <= max {
        return tail;
    }
    clip_to_width(file, max)
}

fn arg_budget(width: u16, failed: bool, base: usize) -> usize {
    let width = usize::from(width.saturating_sub(2)).max(24);
    let cap = width.saturating_sub(8);
    let scaled = if failed {
        cap.saturating_mul(5) / 4
    } else {
        cap
    };
    base.min(scaled.max(20))
}

pub fn clip_to_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let ellipsis_width = ELLIPSIS.width();
    if ellipsis_width >= max {
        return ELLIPSIS.to_owned();
    }
    let mut width = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let char_width = ch.width().unwrap_or(0);
        if width + char_width + ellipsis_width > max {
            break;
        }
        width += char_width;
        out.push(ch);
    }
    out.push_str(ELLIPSIS);
    out
}

#[cfg(test)]
mod tests {
    use super::{ellipsize_path_middle, format, parse, path_under_cwd, transcript_sig};

    #[test]
    fn normalize_wraps_bare_input() {
        assert_eq!(super::normalize("Tool", "value"), "Tool(value)");
    }

    #[test]
    fn parse_keeps_comma_inside_quoted_arg() {
        let sig = format("Tool", &["value, other".to_owned()]);
        let (name, args) = parse("Tool", &sig);
        assert_eq!(name, "Tool");
        assert_eq!(args, ["value, other"]);
    }

    #[test]
    fn transcript_keeps_one_bounded_argument() {
        let sig = transcript_sig(
            "Tool",
            "Tool(first argument, second argument)",
            "/tmp",
            80,
            false,
        );
        assert_eq!(sig, "Tool(\"first argument\", …)");
    }

    #[test]
    fn grep_omits_dot_scope() {
        let sig = transcript_sig("Grep", "Grep(foo, .)", "/tmp", 80, false);
        assert_eq!(sig, "Grep(foo)");
    }

    #[test]
    fn glob_keeps_pattern() {
        let sig = transcript_sig("Glob", "Glob(**/symbols*)", "/tmp", 80, false);
        assert_eq!(sig, "Glob(**/symbols*)");
    }

    #[test]
    fn path_under_cwd_strips_prefix() {
        assert_eq!(
            path_under_cwd("/Users/jmo/proj/crates/a.rs", "/Users/jmo/proj"),
            "crates/a.rs"
        );
    }

    #[test]
    fn call_args_reads_a_bare_display_string() {
        assert_eq!(
            super::call_args("Glob", "Glob(**/symbols*)"),
            vec!["**/symbols*".to_owned()]
        );
    }

    #[test]
    fn path_middle_ellipsis() {
        let path = ellipsize_path_middle("crates/goat-tui/src/transcript/tool_gist.rs", 28);
        assert!(path.contains("tool_gist.rs"));
        assert!(path.contains('…'));
    }

    #[test]
    fn call_args_inverts_call_sig() {
        for input in [
            r#"git commit -m "fix, cleanup""#,
            r#"git add -A && git commit -m "feat: x" && git push"#,
            "cargo nextest run --workspace",
            "echo ''",
        ] {
            let sig = crate::display::call_sig("Tool", &[input]);
            assert_eq!(super::call_args("Tool", &sig), vec![input.to_owned()]);
        }
    }
}

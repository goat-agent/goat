use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const ELLIPSIS: &str = "…";

pub fn transcript_sig(
    tool_name: &str,
    display_primary: &str,
    cwd: &str,
    width: u16,
    failed: bool,
) -> String {
    let ctx = Ctx { cwd, width, failed };
    let full = normalize(tool_name, display_primary);
    let (name, args) = parse(tool_name, &full);
    let args = shorten_args(tool_name, &args, &ctx);
    format(&name, &args)
}

struct Ctx<'a> {
    cwd: &'a str,
    width: u16,
    failed: bool,
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
        let parts: Vec<String> = args
            .iter()
            .map(|a| quote_arg_if_needed(a.as_str()))
            .collect();
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
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        t[1..t.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        t.to_owned()
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

fn shorten_args(tool_name: &str, args: &[String], ctx: &Ctx<'_>) -> Vec<String> {
    if args.is_empty() {
        return Vec::new();
    }
    match tool_name {
        "Read" | "Write" | "Edit" => vec![shorten_path(&args[0], ctx)],
        "Glob" => vec![shorten_text(&args[0], ctx, 64)],
        "Grep" => shorten_grep(args, ctx),
        "Bash" => vec![shorten_command(&args[0], ctx)],
        "WebFetch" => vec![shorten_text(
            &url_host(&args[0]).unwrap_or_else(|| args[0].clone()),
            ctx,
            48,
        )],
        "WebSearch" => vec![shorten_text(&args[0], ctx, 48)],
        "Skill" => vec![shorten_text(&args[0], ctx, 40)],
        "Agent" => shorten_agent(args, ctx),
        "Ask" => vec![shorten_text(&args[0], ctx, 56)],
        _ => shorten_default(args, ctx),
    }
}

fn shorten_grep(args: &[String], ctx: &Ctx<'_>) -> Vec<String> {
    let mut out = vec![shorten_text(&args[0], ctx, 64)];
    if let Some(path) = args.get(1)
        && !path.is_empty()
        && path != "."
    {
        out.push(shorten_path(path, ctx));
    }
    out
}

fn shorten_agent(args: &[String], ctx: &Ctx<'_>) -> Vec<String> {
    if args.len() >= 2 {
        vec![
            shorten_text(&args[0], ctx, 24),
            shorten_text(&args[1], ctx, 40),
        ]
    } else {
        vec![shorten_text(&args[0], ctx, 48)]
    }
}

fn shorten_default(args: &[String], ctx: &Ctx<'_>) -> Vec<String> {
    let mut out = vec![clip_to_budget(&args[0], arg_budget(ctx, 64))];
    if args.len() > 1 {
        out.push("…".to_owned());
    }
    out
}

fn shorten_path(raw: &str, ctx: &Ctx<'_>) -> String {
    let rel = path_under_cwd(raw, ctx.cwd);
    ellipsize_path_middle(&rel, arg_budget(ctx, 56))
}

fn shorten_command(raw: &str, ctx: &Ctx<'_>) -> String {
    let flat: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let cap = if ctx.failed { 72 } else { 56 };
    shorten_text(&flat, ctx, cap)
}

fn shorten_text(s: &str, ctx: &Ctx<'_>, soft_max: usize) -> String {
    let clipped = clip_to_budget(s, arg_budget(ctx, soft_max));
    let budget = arg_budget(ctx, soft_max);
    if clipped.width() > budget {
        clip_to_width(&clipped, budget)
    } else {
        clipped
    }
}

fn clip_to_budget(s: &str, budget: usize) -> String {
    if s.width() <= budget {
        return s.to_owned();
    }
    clip_to_width(s, budget)
}

fn arg_budget(ctx: &Ctx<'_>, base: usize) -> usize {
    let w = usize::from(ctx.width.saturating_sub(2)).max(24);
    let cap = w.saturating_sub(8);
    let scaled = if ctx.failed {
        cap.saturating_mul(5) / 4
    } else {
        cap
    };
    base.min(scaled.max(20))
}

fn path_under_cwd(raw: &str, cwd: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return raw.to_owned();
    }
    let cwd = cwd.trim_end_matches('/');
    if cwd.is_empty() {
        return home_relative(raw);
    }
    let prefix = format!("{cwd}/");
    if let Some(rest) = raw.strip_prefix(&prefix) {
        return rest.to_owned();
    }
    if raw == cwd {
        return ".".to_owned();
    }
    home_relative(raw)
}

fn home_relative(raw: &str) -> String {
    if let Some(home) = std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
        let home = home.trim_end_matches('/');
        if let Some(rest) = raw.strip_prefix(home) {
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            return format!("~/{rest}");
        }
    }
    raw.to_owned()
}

fn ellipsize_path_middle(path: &str, max: usize) -> String {
    if path.width() <= max {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return clip_to_width(path, max);
    }
    let file = parts.last().copied().unwrap_or("");
    let parent = parts.get(parts.len().saturating_sub(2)).copied();
    if let Some(parent) = parent {
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

fn url_host(url: &str) -> Option<String> {
    let t = url.trim();
    let after = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))?;
    let host = after.split('/').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

fn clip_to_width(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let ell_w = ELLIPSIS.width();
    if ell_w >= max {
        return ELLIPSIS.to_owned();
    }
    let mut w = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + ell_w > max {
            break;
        }
        w += cw;
        out.push(ch);
    }
    out.push_str(ELLIPSIS);
    out
}

#[cfg(test)]
mod tests {
    use super::{ellipsize_path_middle, format, parse, path_under_cwd, transcript_sig};

    #[test]
    fn normalize_wraps_bare_pattern() {
        assert_eq!(super::normalize("Glob", "**/symbols*"), "Glob(**/symbols*)");
    }

    #[test]
    fn parse_keeps_comma_inside_quoted_arg() {
        let sig = format("Bash", &["git commit -m \"fix, cleanup\"".to_owned()]);
        let (name, args) = parse("Bash", &sig);
        assert_eq!(name, "Bash");
        assert_eq!(args, ["git commit -m \"fix, cleanup\""]);
    }

    #[test]
    fn read_drops_extra_args() {
        let sig = transcript_sig(
            "Read",
            "Read(/Users/jmo/proj/crates/foo/src/lib.rs, 10, 50)",
            "/Users/jmo/proj",
            100,
            false,
        );
        assert!(sig.starts_with("Read("));
        assert!(sig.contains("crates/foo"));
        assert!(!sig.contains(", 10"));
    }

    #[test]
    fn grep_omits_dot_scope() {
        let sig = transcript_sig("Grep", "Grep(foo, .)", "/tmp", 80, false);
        assert_eq!(sig, "Grep(foo)");
    }

    #[test]
    fn glob_keeps_pattern() {
        let sig = transcript_sig("Glob", "Glob(**/symbols*)", "/x", 80, false);
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
    fn path_middle_ellipsis() {
        let s = ellipsize_path_middle("crates/goat-tui/src/transcript/tool_gist.rs", 28);
        assert!(s.contains("tool_gist.rs"));
        assert!(s.contains('…'));
    }
}

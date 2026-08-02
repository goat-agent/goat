pub(crate) fn to_mrkdwn(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut rest = input;
    let mut at_line_start = true;
    while !rest.is_empty() {
        if let Some(span) = take_code(rest) {
            push_escaped_str(&mut out, span);
            at_line_start = span.ends_with('\n');
            rest = &rest[span.len()..];
            continue;
        }
        let stop = rest.find('`').unwrap_or(rest.len());
        out.push_str(&convert_lines(&rest[..stop], &mut at_line_start));
        rest = &rest[stop..];
    }
    out
}

fn convert_lines(text: &str, at_line_start: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    for chunk in text.split_inclusive('\n') {
        let body = chunk.trim_end_matches('\n');
        let ends_line = chunk.len() != body.len();
        match heading_body(body) {
            Some(title) if *at_line_start && !title.is_empty() => {
                out.push('*');
                out.push_str(&convert_inline(title));
                out.push('*');
            }
            _ => out.push_str(&convert_inline(body)),
        }
        if ends_line {
            out.push('\n');
        }
        *at_line_start = ends_line;
    }
    out
}

fn heading_body(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim())
}

fn take_code(rest: &str) -> Option<&str> {
    if let Some(after) = rest.strip_prefix("```") {
        let end = after.find("```").map_or(rest.len(), |i| 3 + i + 3);
        return Some(&rest[..end]);
    }
    let after = rest.strip_prefix('`')?;
    let end = after.find('`').map_or(rest.len(), |i| 1 + i + 1);
    Some(&rest[..end])
}

fn convert_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        let ch = text[cursor..].chars().next().unwrap_or_default();
        if ch == '['
            && let Some((consumed, rendered)) = take_link(&text[cursor..])
        {
            out.push_str(&rendered);
            cursor += consumed;
            continue;
        }
        if matches!(ch, '*' | '_' | '~')
            && let Some((consumed, rendered)) = take_emphasis(&text[cursor..], ch)
        {
            out.push_str(&rendered);
            cursor += consumed;
            continue;
        }
        push_escaped(&mut out, ch);
        cursor += ch.len_utf8();
    }
    out
}

fn take_emphasis(text: &str, marker: char) -> Option<(usize, String)> {
    let run = text.chars().take_while(|c| *c == marker).count();
    let width = if run >= 2 { 2 } else { 1 };
    let open: String = std::iter::repeat_n(marker, width).collect();
    let body = &text[width..];
    let close = body.find(&open)?;
    let inner = &body[..close];
    if inner.trim().is_empty() {
        return None;
    }
    let rendered = match (marker, width) {
        ('*' | '_', 2) => '*',
        ('*' | '_', 1) => '_',
        ('~', _) => '~',
        _ => return None,
    };
    Some((
        width + close + width,
        format!("{rendered}{}{rendered}", convert_inline(inner)),
    ))
}

fn take_link(text: &str) -> Option<(usize, String)> {
    let close_label = text.find(']')?;
    let after = text.get(close_label + 1..)?;
    if !after.starts_with('(') {
        return None;
    }
    let close_url = after.find(')')?;
    let label = &text[1..close_label];
    let url = after[1..close_url].trim();
    if url.is_empty() || url.starts_with('#') {
        return None;
    }
    let consumed = close_label + 1 + close_url + 1;
    let rendered = if label.trim().is_empty() {
        format!("<{}>", escape_url(url))
    } else {
        format!("<{}|{}>", escape_url(url), convert_inline(label))
    };
    Some((consumed, rendered))
}

fn escape_url(url: &str) -> String {
    url.replace('&', "&amp;")
}

fn push_escaped(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        other => out.push(other),
    }
}

fn push_escaped_str(out: &mut String, text: &str) {
    for ch in text.chars() {
        push_escaped(out, ch);
    }
}

pub(crate) fn strip_mention(text: &str, bot_user_id: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        let Some(end_offset) = rest[start..].find('>') else {
            break;
        };
        let end = start + end_offset + 1;
        let inner = &rest[start + 2..end - 1];
        let id = inner.split('|').next().unwrap_or(inner);
        out.push_str(&rest[..start]);
        if id != bot_user_id {
            out.push_str(&rest[start..end]);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

pub(crate) fn mentions(text: &str, bot_user_id: &str) -> bool {
    let needle = format!("<@{bot_user_id}");
    text.match_indices(&needle).any(|(index, _)| {
        text[index + needle.len()..]
            .chars()
            .next()
            .is_some_and(|next| next == '>' || next == '|')
    })
}

pub(crate) fn from_mrkdwn(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        let Some(end_offset) = rest[start..].find('>') else {
            break;
        };
        let end = start + end_offset + 1;
        out.push_str(&unescape(&rest[..start]));
        out.push_str(&unwrap_entity(&rest[start + 1..end - 1]));
        rest = &rest[end..];
    }
    out.push_str(&unescape(rest));
    out
}

fn unwrap_entity(inner: &str) -> String {
    let (target, label) = match inner.split_once('|') {
        Some((target, label)) => (target, Some(unescape(label))),
        None => (inner, None),
    };
    if let Some(user) = target.strip_prefix('@') {
        return format!("@{}", label.unwrap_or_else(|| user.to_string()));
    }
    if let Some(channel) = target.strip_prefix('#') {
        return format!("#{}", label.unwrap_or_else(|| channel.to_string()));
    }
    if let Some(special) = target.strip_prefix('!') {
        let name = special.split('^').next().unwrap_or(special);
        return label.unwrap_or_else(|| format!("@{name}"));
    }
    let target = unescape(target);
    match label {
        Some(label) if label != target => format!("{label} ({target})"),
        _ => target,
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_collapses_to_a_single_asterisk() {
        assert_eq!(to_mrkdwn("**bold**"), "*bold*");
        assert_eq!(to_mrkdwn("__bold__"), "*bold*");
        assert_eq!(to_mrkdwn("a **b** c"), "a *b* c");
    }

    #[test]
    fn italic_becomes_an_underscore() {
        assert_eq!(to_mrkdwn("*italic*"), "_italic_");
        assert_eq!(to_mrkdwn("_italic_"), "_italic_");
    }

    #[test]
    fn strikethrough_collapses_to_a_single_tilde() {
        assert_eq!(to_mrkdwn("~~gone~~"), "~gone~");
        assert_eq!(to_mrkdwn("~gone~"), "~gone~");
    }

    #[test]
    fn nested_emphasis_converts_inside_out() {
        assert_eq!(to_mrkdwn("**bold _and_ more**"), "*bold _and_ more*");
        assert_eq!(to_mrkdwn("**bold *inner* tail**"), "*bold _inner_ tail*");
    }

    #[test]
    fn links_become_angle_pipe_form() {
        assert_eq!(
            to_mrkdwn("[goat](https://example.com)"),
            "<https://example.com|goat>"
        );
        assert_eq!(
            to_mrkdwn("see [the docs](https://example.com/a_b) now"),
            "see <https://example.com/a_b|the docs> now"
        );
    }

    #[test]
    fn a_link_with_no_label_keeps_only_the_url() {
        assert_eq!(
            to_mrkdwn("[](https://example.com)"),
            "<https://example.com>"
        );
    }

    #[test]
    fn an_anchor_only_link_is_left_alone() {
        assert_eq!(to_mrkdwn("[top](#heading)"), "[top](#heading)");
    }

    #[test]
    fn ampersands_in_a_url_are_escaped() {
        assert_eq!(
            to_mrkdwn("[q](https://example.com/?a=1&b=2)"),
            "<https://example.com/?a=1&amp;b=2|q>"
        );
    }

    #[test]
    fn headings_become_bold_lines() {
        assert_eq!(to_mrkdwn("# Title"), "*Title*");
        assert_eq!(to_mrkdwn("### Deep  "), "*Deep*");
        assert_eq!(to_mrkdwn("## A\nbody\n"), "*A*\nbody\n");
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        assert_eq!(to_mrkdwn("#hashtag"), "#hashtag");
        assert_eq!(to_mrkdwn("####### too deep"), "####### too deep");
    }

    #[test]
    fn inline_code_is_passed_through_untouched() {
        assert_eq!(
            to_mrkdwn("use `**not bold**` here"),
            "use `**not bold**` here"
        );
        assert_eq!(to_mrkdwn("`[a](b)`"), "`[a](b)`");
    }

    #[test]
    fn fenced_code_is_passed_through_untouched() {
        let input = "before\n```rust\nlet x = **y**;\n# not a heading\n```\nafter **b**";
        let expected = "before\n```rust\nlet x = **y**;\n# not a heading\n```\nafter *b*";
        assert_eq!(to_mrkdwn(input), expected);
    }

    #[test]
    fn an_unclosed_fence_still_terminates() {
        assert_eq!(to_mrkdwn("```\nx = 1"), "```\nx = 1");
    }

    #[test]
    fn slack_control_characters_are_escaped_in_prose() {
        assert_eq!(to_mrkdwn("a < b && c > d"), "a &lt; b &amp;&amp; c &gt; d");
    }

    #[test]
    fn control_characters_are_escaped_inside_code_too() {
        assert_eq!(to_mrkdwn("`a < b`"), "`a &lt; b`");
    }

    #[test]
    fn unmatched_markers_survive_as_literal_text() {
        assert_eq!(to_mrkdwn("2 * 3 = 6"), "2 * 3 = 6");
        assert_eq!(to_mrkdwn("**unclosed"), "**unclosed");
        assert_eq!(to_mrkdwn("* *"), "* *");
    }

    #[test]
    fn snake_case_identifiers_are_not_corrupted() {
        assert_eq!(to_mrkdwn("some_var_name"), "some_var_name");
    }

    #[test]
    fn bullet_lists_are_left_alone() {
        assert_eq!(to_mrkdwn("- one\n- two"), "- one\n- two");
    }

    #[test]
    fn strip_mention_removes_only_the_bot() {
        assert_eq!(strip_mention("<@UBOT> hello", "UBOT"), "hello");
        assert_eq!(strip_mention("<@UBOT|goat> hi", "UBOT"), "hi");
        assert_eq!(strip_mention("hey <@UBOT>", "UBOT"), "hey");
        assert_eq!(
            strip_mention("<@UBOT> ping <@UOTHER>", "UBOT"),
            "ping <@UOTHER>"
        );
        assert_eq!(strip_mention("<@UOTHER> only", "UBOT"), "<@UOTHER> only");
    }

    #[test]
    fn strip_mention_tolerates_a_broken_entity() {
        assert_eq!(
            strip_mention("<@UBOT unterminated", "UBOT"),
            "<@UBOT unterminated"
        );
    }

    #[test]
    fn mentions_matches_the_whole_id_only() {
        assert!(mentions("<@UBOT> hi", "UBOT"));
        assert!(mentions("<@UBOT|goat> hi", "UBOT"));
        assert!(!mentions("<@UBOTTOM> hi", "UBOT"));
        assert!(!mentions("plain text", "UBOT"));
    }

    #[test]
    fn inbound_user_and_channel_entities_are_unwrapped() {
        assert_eq!(from_mrkdwn("<@U123> hi"), "@U123 hi");
        assert_eq!(from_mrkdwn("<@U123|jmo> hi"), "@jmo hi");
        assert_eq!(from_mrkdwn("in <#C123|general>"), "in #general");
        assert_eq!(from_mrkdwn("in <#C123>"), "in #C123");
    }

    #[test]
    fn inbound_special_mentions_are_readable() {
        assert_eq!(from_mrkdwn("<!here> ping"), "@here ping");
        assert_eq!(from_mrkdwn("<!channel>"), "@channel");
        assert_eq!(from_mrkdwn("<!subteam^S1|@eng>"), "@eng");
    }

    #[test]
    fn inbound_links_keep_both_label_and_target() {
        assert_eq!(
            from_mrkdwn("see <https://example.com|the docs>"),
            "see the docs (https://example.com)"
        );
        assert_eq!(from_mrkdwn("<https://example.com>"), "https://example.com");
        assert_eq!(
            from_mrkdwn("<https://example.com|https://example.com>"),
            "https://example.com"
        );
    }

    #[test]
    fn inbound_escapes_are_undone() {
        assert_eq!(
            from_mrkdwn("a &lt; b &amp;&amp; c &gt; d"),
            "a < b && c > d"
        );
    }

    #[test]
    fn inbound_tolerates_an_unterminated_entity() {
        assert_eq!(from_mrkdwn("a < b"), "a < b");
    }

    #[test]
    fn escaping_round_trips_for_prose() {
        let original = "a < b && c > d";
        assert_eq!(from_mrkdwn(&to_mrkdwn(original)), original);
    }
}

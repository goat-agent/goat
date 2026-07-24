use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    highlight::Highlighter, layout::format_tokens, markdown, overlay::truncate_to_width, symbols,
    theme::Theme, wrap,
};

use super::ImagePlacement;
use super::item::{Item, ShellStatus, ToolStatus, Working};
use super::tool_gist::ToolLineCtx;
use super::tool_line::{ToolRowInput, tool_marker, tool_row};

pub(super) fn is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}

pub(super) fn stable_prefix_len(buffer: &str) -> usize {
    let mut in_fence = false;
    let mut offset = 0usize;
    let mut split = 0usize;
    let mut prev_blank = true;
    for line in buffer.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let stripped = trimmed.trim_start();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let is_blank = trimmed.trim().is_empty();
        if is_blank && !in_fence && !prev_blank {
            split = offset + line.len();
        }
        prev_blank = is_blank;
        offset += line.len();
    }
    split.min(buffer.len())
}

pub(super) use super::gutter::hang;

pub(super) fn plain_lines(text: &str, theme: Theme) -> Vec<Line<'static>> {
    text.split('\n')
        .map(|raw| Line::from(Span::styled(raw.to_owned(), theme.base())))
        .collect()
}

fn plain_lines_styled(text: &str, style: ratatui::style::Style) -> Vec<Line<'static>> {
    text.split('\n')
        .map(|raw| Line::from(Span::styled(raw.to_owned(), style)))
        .collect()
}

const INTERRUPT_AGENT_SUFFIX: &str = "\n\n(interrupted)";

fn agent_lines_with_optional_interrupt_suffix(
    text: &str,
    theme: Theme,
    hl: &dyn Highlighter,
) -> Vec<Line<'static>> {
    if let Some(body) = text.strip_suffix(INTERRUPT_AGENT_SUFFIX) {
        let mut lines = if body.is_empty() {
            Vec::new()
        } else {
            let rendered = markdown::render(body, theme, hl);
            let end = rendered
                .iter()
                .rposition(|l| !is_blank(l))
                .map_or(0, |i| i + 1);
            rendered[..end].to_vec()
        };
        lines.push(Line::from(Span::styled(
            "(interrupted)".to_owned(),
            theme.error_body(),
        )));
        return lines;
    }
    let rendered = markdown::render(text, theme, hl);
    let end = rendered
        .iter()
        .rposition(|l| !is_blank(l))
        .map_or(0, |i| i + 1);
    rendered[..end].to_vec()
}

fn line_width(line: &Line<'static>) -> usize {
    line.spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn user_panel_rows(mut rows: Vec<Line<'static>>, theme: Theme, width: u16) -> Vec<Line<'static>> {
    let panel = theme.user_panel();
    let target = usize::from(width);
    for line in &mut rows {
        for span in &mut line.spans {
            span.style = span.style.patch(panel);
        }
        let pad = target.saturating_sub(line_width(line));
        if pad > 0 {
            line.spans.push(Span::styled(" ".repeat(pad), panel));
        }
    }
    rows
}

pub(super) fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

const QUEUED_ROW_CAP: usize = 3;

pub(super) fn queued_rows(theme: Theme, width: u16, queued: &[String]) -> Vec<Line<'static>> {
    let inner = usize::from(width.saturating_sub(2));
    let mut rows: Vec<Line<'static>> = Vec::new();
    for label in queued.iter().take(QUEUED_ROW_CAP) {
        rows.push(Line::from(vec![
            Span::styled(symbols::marker::USER, theme.muted()),
            Span::styled(truncate_to_width(label, inner), theme.muted()),
        ]));
    }
    if queued.len() > QUEUED_ROW_CAP {
        rows.push(Line::from(Span::styled(
            format!(
                "{} {} more queued",
                symbols::ui::ELLIPSIS,
                queued.len() - QUEUED_ROW_CAP
            ),
            theme.muted(),
        )));
    }
    rows
}

pub(super) fn working_rows(
    theme: Theme,
    width: u16,
    spinner: &'static str,
    w: &Working,
) -> Vec<Line<'static>> {
    let label = w.label.clone().unwrap_or_else(|| {
        let verb = if w.thinking { "thinking" } else { "working" };
        format!("{verb}{}", symbols::ui::ELLIPSIS)
    });
    let mut spans = vec![Span::styled(label, theme.muted())];
    if let Some(secs) = w.elapsed {
        spans.push(Span::styled(
            format!("{}{}", symbols::ui::SEPARATOR, format_elapsed(secs)),
            theme.muted(),
        ));
    }
    if let Some(tokens) = w.tokens {
        spans.push(Span::styled(
            format!("{}{} tok", symbols::ui::SEPARATOR, format_tokens(tokens)),
            theme.muted(),
        ));
    }
    hang(
        &[Line::from(spans)],
        Span::styled(format!("{spinner} "), theme.accent()),
        width,
    )
}

pub(super) struct ItemMemo {
    pub(super) sig: u64,
    pub(super) rows: Vec<Line<'static>>,
}

pub(super) fn build_static_lines(
    items: &[Item],
    theme: Theme,
    width: u16,
    hl: &dyn Highlighter,
    cwd: &str,
    memo: &mut Vec<ItemMemo>,
) -> (Vec<Line<'static>>, Vec<usize>, Vec<ImagePlacement>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spinner_lines: Vec<usize> = Vec::new();
    let mut images: Vec<ImagePlacement> = Vec::new();
    if memo.len() > items.len() {
        memo.truncate(items.len());
    }
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            let prev_is_tool = matches!(items.get(i - 1), Some(Item::Tool { .. }));
            let cur_is_tool = matches!(item, Item::Tool { .. });
            if !(prev_is_tool && cur_is_tool) {
                lines.push(Line::default());
            }
        }
        if matches!(
            item,
            Item::Tool {
                status: ToolStatus::Running,
                ..
            } | Item::Shell {
                status: ShellStatus::Running,
                ..
            } | Item::Process { running: true, .. }
        ) {
            spinner_lines.push(lines.len());
        }
        let sig = item_signature(item);
        let rows = match memo.get(i) {
            Some(cached) if cached.sig == sig => cached.rows.clone(),
            _ => {
                let rows = item_rows(item, theme, width, hl, cwd);
                let entry = ItemMemo {
                    sig,
                    rows: rows.clone(),
                };
                if i < memo.len() {
                    memo[i] = entry;
                } else {
                    memo.push(entry);
                }
                rows
            }
        };
        lines.extend(rows);
        if let Item::Tool {
            image: Some(img), ..
        } = item
        {
            let rows = img.rows();
            if rows > 0 {
                images.push(ImagePlacement {
                    item: i,
                    start: lines.len(),
                    rows,
                });
                for _ in 0..rows {
                    lines.push(Line::default());
                }
            }
        }
    }
    (lines, spinner_lines, images)
}

pub(super) fn item_signature(item: &Item) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match item {
        Item::User(message) => {
            0u8.hash(&mut hasher);
            message.text.hash(&mut hasher);
            for attachment in &message.attachments {
                attachment.label.hash(&mut hasher);
                attachment.media_type.hash(&mut hasher);
                attachment.data.hash(&mut hasher);
            }
        }
        Item::Agent(text) => {
            1u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        Item::Thinking { text, collapsed } => {
            8u8.hash(&mut hasher);
            text.hash(&mut hasher);
            collapsed.hash(&mut hasher);
        }
        Item::Shell {
            command, status, ..
        } => {
            2u8.hash(&mut hasher);
            command.hash(&mut hasher);
            match status {
                ShellStatus::Running => 0u8.hash(&mut hasher),
                ShellStatus::Done(output) => {
                    1u8.hash(&mut hasher);
                    output.hash(&mut hasher);
                }
            }
        }
        Item::Process {
            command,
            output,
            running,
            exit_code,
        } => {
            9u8.hash(&mut hasher);
            command.hash(&mut hasher);
            output.hash(&mut hasher);
            running.hash(&mut hasher);
            exit_code.hash(&mut hasher);
        }
        Item::Error { message, hint } => {
            3u8.hash(&mut hasher);
            message.hash(&mut hasher);
            hint.hash(&mut hasher);
        }
        Item::Interrupted => {
            7u8.hash(&mut hasher);
        }
        Item::Compaction {
            tokens_before,
            tokens_after,
        } => {
            5u8.hash(&mut hasher);
            tokens_before.hash(&mut hasher);
            tokens_after.hash(&mut hasher);
        }
        Item::Tool {
            name,
            display,
            status,
            ..
        } => {
            6u8.hash(&mut hasher);
            name.hash(&mut hasher);
            display.primary.hash(&mut hasher);
            match status {
                ToolStatus::Running => 0u8.hash(&mut hasher),
                ToolStatus::Done(outcome) => {
                    1u8.hash(&mut hasher);
                    outcome.ok.hash(&mut hasher);
                    outcome.summary.hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

pub(super) fn item_rows(
    item: &Item,
    theme: Theme,
    width: u16,
    hl: &dyn Highlighter,
    cwd: &str,
) -> Vec<Line<'static>> {
    match item {
        Item::User(message) => {
            let mut lines = plain_lines(&message.text, theme);
            for attachment in &message.attachments {
                lines.push(Line::from(Span::styled(
                    format!("[image: {}]", attachment.label),
                    theme.muted(),
                )));
            }
            let rows = hang(
                &lines,
                Span::styled(symbols::marker::USER, theme.role_user()),
                width,
            );
            user_panel_rows(rows, theme, width)
        }
        Item::Agent(text) => {
            let rendered = agent_lines_with_optional_interrupt_suffix(text, theme, hl);
            hang(
                &rendered,
                Span::styled(symbols::marker::AGENT, theme.role_agent()),
                width,
            )
        }
        Item::Thinking { text, collapsed } => thinking_rows(text, *collapsed, theme, width),
        Item::Shell {
            command, status, ..
        } => shell_rows(command, status, theme, width),
        Item::Process {
            command,
            output,
            running,
            exit_code,
        } => process_rows(command, output, *running, *exit_code, theme, width),
        Item::Error { message, hint } => error_rows(message, hint.as_deref(), theme, width),
        Item::Interrupted => {
            let inner = width.saturating_sub(2);
            let line = Line::from(vec![
                Span::styled("Turn ", theme.base()),
                Span::styled("interrupted.", theme.error_body()),
            ]);
            let wrapped = wrap::wrap_line(&line, inner);
            hang(
                &wrapped,
                Span::styled(symbols::marker::ERROR, theme.error()),
                width,
            )
        }
        Item::Compaction {
            tokens_before,
            tokens_after,
        } => {
            let label = format!(
                " context compacted{}{} → {} ",
                symbols::ui::SEPARATOR,
                format_tokens(u64::from(*tokens_before)),
                format_tokens(u64::from(*tokens_after)),
            );
            let total = usize::from(width).saturating_sub(2);
            let dashes = total.saturating_sub(UnicodeWidthStr::width(label.as_str()));
            let left = dashes / 2;
            let right = dashes - left;
            vec![Line::from(vec![
                Span::raw("  "),
                Span::styled("─".repeat(left), theme.muted()),
                Span::styled(label, theme.muted()),
                Span::styled("─".repeat(right), theme.muted()),
            ])]
        }
        Item::Tool {
            name,
            display,
            status,
            ..
        } => {
            let (marker, marker_style) = tool_marker(status, theme);
            let failed = matches!(status, ToolStatus::Done(o) if !o.ok);
            let mut rows = tool_row(&ToolRowInput {
                name,
                display_primary: &display.primary,
                marker,
                marker_style,
                theme,
                width,
                line_ctx: ToolLineCtx { cwd, width, failed },
            });
            if let ToolStatus::Done(outcome) = status
                && outcome.ok
                && let Some(summary) = outcome.summary.as_deref()
                && is_diff_summary(summary)
            {
                rows.extend(diff_body_rows(summary, theme, width));
            }
            rows
        }
    }
}

pub(super) const SHELL_BLOCK_CAP: usize = 20;
const SHELL_EXIT_PREFIX: &str = "exit code: ";
const SHELL_NO_OUTPUT: &str = "(no output)";

pub(super) fn resolve_carriage_returns(line: &str) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.rsplit('\r').next().unwrap_or(line)
}

pub(super) fn strip_control_sequences(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub(super) fn sanitize_shell_output(output: &str) -> Vec<String> {
    let mut lines: Vec<String> = output
        .split('\n')
        .map(|line| strip_control_sequences(resolve_carriage_returns(line)))
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

pub(super) fn shell_line_style(line: &str, theme: Theme) -> Style {
    if line.starts_with(SHELL_EXIT_PREFIX) || (line.starts_with('[') && line.ends_with(']')) {
        theme.error()
    } else if line.starts_with("+ ") {
        theme.role_agent()
    } else if line.starts_with("- ") {
        theme.error()
    } else {
        theme.text()
    }
}

pub(super) fn is_diff_summary(summary: &str) -> bool {
    summary
        .lines()
        .any(|line| line.starts_with("+ ") || line.starts_with("- "))
}

pub(super) fn diff_body_rows(summary: &str, theme: Theme, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in summary.split('\n') {
        let content = Line::from(Span::styled(line.to_owned(), diff_line_style(line, theme)));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans.insert(0, Span::raw("  "));
            rows.push(row);
        }
    }
    rows
}

fn diff_line_style(line: &str, theme: Theme) -> Style {
    if line.starts_with("+ ") {
        theme.role_agent()
    } else if line.starts_with("- ") {
        theme.error()
    } else {
        theme.muted()
    }
}

fn error_rows(text: &str, hint: Option<&str>, theme: Theme, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in plain_lines_styled(text, theme.error_body()) {
        for mut row in wrap::wrap_line(&line, inner) {
            let gutter = if out.is_empty() {
                symbols::marker::ERROR
            } else {
                symbols::ui::QUOTE_GUTTER
            };
            row.spans.insert(0, Span::styled(gutter, theme.error()));
            out.push(row);
        }
    }
    if out.is_empty() {
        out.push(Line::from(Span::styled(
            symbols::marker::ERROR,
            theme.error(),
        )));
    }
    if let Some(hint) = hint {
        let line = Line::from(hint_spans(hint, theme));
        for mut row in wrap::wrap_line(&line, inner) {
            row.spans
                .insert(0, Span::styled(symbols::ui::QUOTE_GUTTER, theme.error()));
            out.push(row);
        }
    }
    out
}

fn thinking_rows(text: &str, collapsed: bool, theme: Theme, width: u16) -> Vec<Line<'static>> {
    let marker = if collapsed {
        symbols::ui::CHEVRON_RIGHT
    } else {
        symbols::ui::CHEVRON_DOWN
    };
    let header = Line::from(vec![
        Span::styled(format!("{marker} "), theme.muted()),
        Span::styled("Thought", theme.muted()),
    ]);
    if collapsed {
        return vec![header];
    }
    let inner = width.saturating_sub(2);
    let mut out = vec![header];
    let body = text.trim_end();
    for line in body.split('\n') {
        let content = Line::from(Span::styled(line.to_owned(), theme.muted()));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans
                .insert(0, Span::styled(symbols::ui::QUOTE_GUTTER, theme.muted()));
            out.push(row);
        }
    }
    out
}

fn hint_spans(hint: &str, theme: Theme) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("{} ", symbols::key::ARROW_RIGHT),
        theme.muted(),
    )];
    for (i, word) in hint.split(' ').enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", theme.muted()));
        }
        let style = if word.starts_with('/') {
            theme.accent()
        } else {
            theme.muted()
        };
        spans.push(Span::styled(word.to_owned(), style));
    }
    spans
}

pub(super) fn shell_rows(
    command: &str,
    status: &ShellStatus,
    theme: Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let (marker, marker_style) = match status {
        ShellStatus::Running => (symbols::SPINNER[0], theme.accent()),
        ShellStatus::Done(_) => (symbols::ui::BANG, theme.shell()),
    };
    let mut out = hang(
        &plain_lines(command, theme),
        Span::styled(format!("{marker} "), marker_style),
        width,
    );

    let ShellStatus::Done(output) = status else {
        return out;
    };
    let lines = sanitize_shell_output(output);
    if lines.is_empty() {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(SHELL_NO_OUTPUT, theme.muted()),
        ]));
        return out;
    }
    let exit_line = lines
        .last()
        .filter(|line| line.starts_with(SHELL_EXIT_PREFIX))
        .cloned();
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in &lines {
        let content = Line::from(Span::styled(
            line.replace('\t', "  "),
            shell_line_style(line, theme),
        ));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans.insert(0, Span::raw("  "));
            rows.push(row);
        }
    }
    let total = rows.len();
    if total > SHELL_BLOCK_CAP {
        rows.truncate(SHELL_BLOCK_CAP);
        rows.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} {} more", symbols::ui::ELLIPSIS, total - SHELL_BLOCK_CAP),
                theme.muted(),
            ),
        ]));
        if let Some(exit) = exit_line {
            rows.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(exit, theme.error()),
            ]));
        }
    }
    out.extend(rows);
    out
}

pub(super) fn process_rows(
    command: &str,
    output: &str,
    running: bool,
    exit_code: Option<i32>,
    theme: Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let (marker, marker_style) = if running {
        (symbols::SPINNER[0], theme.accent())
    } else if exit_code == Some(0) || exit_code.is_none() {
        (symbols::ui::CHECK, theme.success())
    } else {
        (symbols::ui::CROSS, theme.error())
    };
    let mut out = hang(
        &plain_lines(command, theme),
        Span::styled(format!("{marker} "), marker_style),
        width,
    );
    let lines = sanitize_shell_output(output);
    for line in &lines {
        let content = Line::from(Span::styled(
            line.replace('\t', "  "),
            shell_line_style(line, theme),
        ));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans.insert(0, Span::raw("  "));
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{diff_body_rows, is_diff_summary};
    use crate::theme::Theme;

    #[test]
    fn is_diff_summary_detects_change_lines() {
        assert!(is_diff_summary("- old\n+ new"));
        assert!(is_diff_summary("3 replacements\n- a\n+ b"));
        assert!(!is_diff_summary("1 line"));
        assert!(!is_diff_summary("wrote out.txt"));
    }

    #[test]
    fn diff_body_rows_color_change_lines() {
        let theme = Theme::dark();
        let rows = diff_body_rows("- world\n+ there", theme, 40);
        assert_eq!(rows.len(), 2);
        let removed = &rows[0];
        let added = &rows[1];
        assert!(removed.spans.iter().any(|s| s.content.contains("- world")));
        assert!(added.spans.iter().any(|s| s.content.contains("+ there")));
        let removed_style = removed.spans.last().unwrap().style;
        let added_style = added.spans.last().unwrap().style;
        assert_eq!(removed_style.fg, theme.error().fg);
        assert_eq!(added_style.fg, theme.role_agent().fg);
    }

    #[test]
    fn diff_body_rows_indent_two_columns() {
        let rows = diff_body_rows("- x", Theme::dark(), 40);
        assert_eq!(rows[0].spans[0].content.as_ref(), "  ");
    }

    #[test]
    fn error_rows_rail_on_continuation() {
        use super::{error_rows, symbols};
        let rows = error_rows("line one\nline two\nline three", None, Theme::dark(), 60);
        assert_eq!(rows[0].spans[0].content.as_ref(), symbols::marker::ERROR);
        assert_eq!(rows[1].spans[0].content.as_ref(), symbols::ui::QUOTE_GUTTER);
        assert_eq!(rows[2].spans[0].content.as_ref(), symbols::ui::QUOTE_GUTTER);
    }

    #[test]
    fn thinking_rows_collapsed_is_single_line() {
        use super::{symbols, thinking_rows};
        let rows = thinking_rows("some reasoning\nmore", true, Theme::dark(), 60);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].spans[0]
                .content
                .contains(symbols::ui::CHEVRON_RIGHT)
        );
        assert!(rows[0].spans.iter().any(|s| s.content.contains("Thought")));
    }

    #[test]
    fn thinking_rows_expanded_shows_body_with_gutter() {
        use super::{symbols, thinking_rows};
        let rows = thinking_rows("line a\nline b", false, Theme::dark(), 60);
        assert!(rows[0].spans[0].content.contains(symbols::ui::CHEVRON_DOWN));
        assert!(rows.len() >= 3);
        assert_eq!(rows[1].spans[0].content.as_ref(), symbols::ui::QUOTE_GUTTER);
    }

    #[test]
    fn error_rows_render_action_hint_with_command_accent() {
        use super::{error_rows, symbols};
        let theme = Theme::dark();
        let rows = error_rows("auth failed", Some("/config to re-login"), theme, 60);
        let action = rows.last().unwrap();
        assert_eq!(action.spans[0].content.as_ref(), symbols::ui::QUOTE_GUTTER);
        assert!(action.spans.iter().any(|s| s.content.contains('→')));
        let command = action
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "/config")
            .expect("slash command span present");
        assert_eq!(command.style.fg, theme.accent().fg);
    }
}

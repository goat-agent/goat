use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{symbols, theme::Theme};

pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for c in s.chars() {
        let char_width = c.width().unwrap_or(0);
        if width + char_width + 1 > max_width {
            break;
        }
        out.push(c);
        width += char_width;
    }
    out.push_str(symbols::ui::ELLIPSIS);
    out
}

pub fn clamp_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

pub fn overlay_frame(frame: &mut Frame, area: Rect, theme: Theme) -> Option<Rect> {
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border())
        .style(theme.base());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    Some(inner)
}

pub fn ask_sheet_frame(frame: &mut Frame, area: Rect, theme: Theme) -> Option<Rect> {
    overlay_frame(frame, area, theme)
}

pub fn overlay_layout(inner: Rect) -> (Rect, Rect, Rect) {
    let [context, body, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    (context, body, hint)
}

pub fn overlay_layout_plain(inner: Rect) -> (Rect, Rect) {
    let [body, hint] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    (body, hint)
}

pub fn hint_line<'a>(pairs: &[(&'a str, &'a str)], theme: Theme) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = vec![Span::raw(" ")];
    for (i, (glyph, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*glyph, theme.hint_key()));
        spans.push(Span::styled(format!(" {label}"), theme.muted()));
    }
    Line::from(spans)
}

pub fn selection_row<'a>(
    theme: Theme,
    selected: bool,
    inner_width: usize,
    left_spans: Vec<Span<'a>>,
    right_span: Option<Span<'a>>,
) -> Line<'a> {
    let caret = if selected {
        Span::styled(format!(" {} ", symbols::ui::CARET), theme.accent())
    } else {
        Span::raw("   ")
    };
    let caret_w = 3usize;
    let right_w = right_span.as_ref().map_or(0, |s| s.content.width());
    let gap = usize::from(right_span.is_some());
    let left_budget = inner_width.saturating_sub(caret_w + right_w + gap);
    let mut left_w = 0usize;
    let mut fitted: Vec<Span<'a>> = Vec::new();
    for span in left_spans {
        let content = span.content.as_ref();
        let w = content.width();
        if left_w + w <= left_budget {
            left_w += w;
            fitted.push(span);
            continue;
        }
        let remaining = left_budget.saturating_sub(left_w);
        if remaining > 0 {
            let truncated = truncate_to_width(content, remaining);
            left_w += truncated.width();
            fitted.push(Span::styled(truncated, span.style));
        }
        break;
    }
    let pad = inner_width.saturating_sub(caret_w + left_w + right_w + gap);
    let mut spans = vec![caret];
    spans.extend(fitted);
    spans.push(Span::raw(" ".repeat(pad)));
    if let Some(right) = right_span {
        spans.push(right);
    }
    Line::from(spans)
}

pub fn overflow_hint(start: usize, shown: usize, total: usize) -> (Option<String>, Option<String>) {
    let above = if start > 0 {
        Some(format!("{} {} more", symbols::ui::MORE_ABOVE, start))
    } else {
        None
    };
    let remaining = total.saturating_sub(start + shown);
    let below = if remaining > 0 {
        Some(format!("{} {} more", symbols::ui::MORE_BELOW, remaining))
    } else {
        None
    };
    (above, below)
}

pub struct Window {
    pub start: usize,
    pub shown: usize,
    pub above: Option<String>,
    pub below: Option<String>,
}

pub fn window(cursor: usize, len: usize, rows: usize) -> Window {
    if rows == 0 || len == 0 {
        return Window {
            start: 0,
            shown: 0,
            above: None,
            below: None,
        };
    }
    let needs_above = len > rows;
    let needs_below = len > rows;
    let body = rows
        .saturating_sub(usize::from(needs_above))
        .saturating_sub(usize::from(needs_below))
        .max(1);
    let mut start = if cursor >= body { cursor + 1 - body } else { 0 };
    start = start.min(len.saturating_sub(body));
    let shown = body.min(len.saturating_sub(start));
    let (above, below) = overflow_hint(start, shown, len);
    Window {
        start,
        shown,
        above,
        below,
    }
}

pub fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = max_width.min(area.width.saturating_sub(4));
    let height = max_height.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::text::Span;
    use unicode_width::UnicodeWidthStr;

    use super::selection_row;
    use crate::theme::Theme;

    #[test]
    fn selected_row_has_no_background_band() {
        let theme = Theme::dark();
        let line = selection_row(theme, true, 40, vec![Span::raw("item")], None);
        assert!(
            line.spans.iter().all(|span| span.style.bg.is_none()),
            "selection must not paint a full-width background band"
        );
    }

    #[test]
    fn selection_row_truncates_overflow() {
        let theme = Theme::dark();
        let line = selection_row(
            theme,
            true,
            12,
            vec![Span::raw("very long label")],
            Some(Span::raw("right")),
        );
        let width: usize = line.spans.iter().map(|s| s.content.width()).sum();
        assert!(width <= 12);
    }
}

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::{symbols, wrap};

fn leading_indent(line: &Line<'static>) -> Vec<Span<'static>> {
    let mut prefix: Vec<Span<'static>> = Vec::new();
    let gutter_ch = symbols::ui::QUOTE_GUTTER.chars().next().unwrap_or('▎');
    for span in &line.spans {
        let content = span.content.as_ref();
        let is_blank = !content.is_empty() && content.chars().all(|c| c == ' ');
        let is_gutter = content.starts_with(gutter_ch);
        if is_blank {
            prefix.push(Span::styled(content.to_owned(), span.style));
        } else if is_gutter {
            prefix.push(span.clone());
            break;
        } else {
            break;
        }
    }
    prefix
}

pub(crate) fn hang(
    content: &[Line<'static>],
    marker: Span<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let mut first = Some(marker);
    if content.is_empty() {
        return vec![Line::from(vec![first.take().unwrap_or_default()])];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in content {
        if line.spans.len() == 1 && line.spans[0].content.as_ref() == symbols::ui::HRULE {
            let style = line.spans[0].style;
            let prefix = first.take().unwrap_or_else(|| Span::raw("  "));
            let prefix_w = UnicodeWidthStr::width(prefix.content.as_ref());
            let rule_w = usize::from(width).saturating_sub(prefix_w).max(1);
            out.push(Line::from(vec![
                prefix,
                Span::styled("─".repeat(rule_w), style),
            ]));
            continue;
        }
        let indent = leading_indent(line);
        let mut wrapped = wrap::wrap_line(line, inner).into_iter();
        if let Some(mut first_row) = wrapped.next() {
            let prefix = first.take().unwrap_or_else(|| Span::raw("  "));
            first_row.spans.insert(0, prefix);
            out.push(first_row);
        }
        for mut row in wrapped {
            for (i, span) in indent.iter().enumerate() {
                row.spans.insert(i, span.clone());
            }
            row.spans.insert(0, Span::raw("  "));
            out.push(row);
        }
    }
    out
}

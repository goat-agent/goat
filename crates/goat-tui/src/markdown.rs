use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{highlight::Highlighter, symbols, theme::Theme};

pub fn render(md: &str, theme: Theme, hl: &dyn Highlighter) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let mut state = RenderState::new(theme, hl);
    for event in Parser::new_ext(md, opts) {
        state.handle_event(event);
    }
    state.finish()
}

#[derive(Default, Clone, Copy)]
struct Emphasis {
    bold: bool,
    italic: bool,
    strikethrough: bool,
}

impl Emphasis {
    fn style(self, theme: Theme) -> Style {
        let mut style = theme.base();
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strikethrough {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        style
    }
}

struct RenderState<'a> {
    theme: Theme,
    hl: &'a dyn Highlighter,
    lines: Vec<Line<'static>>,
    current_spans: Vec<Span<'static>>,
    emphasis: Emphasis,
    in_code_block: bool,
    blockquote_depth: usize,
    code_lang: String,
    code_buf: String,
    list_stack: Vec<Option<u64>>,
    list_item_index: Vec<u64>,
    link_url: Option<String>,
    link_text_start: usize,
    table_headers: Vec<Vec<Span<'static>>>,
    table_rows: Vec<Vec<Vec<Span<'static>>>>,
    current_row: Vec<Vec<Span<'static>>>,
    current_cell: Vec<Span<'static>>,
    in_table: bool,
    in_thead: bool,
    col_idx: usize,
}

impl<'a> RenderState<'a> {
    fn new(theme: Theme, hl: &'a dyn Highlighter) -> Self {
        Self {
            theme,
            hl,
            lines: Vec::new(),
            current_spans: Vec::new(),
            emphasis: Emphasis::default(),
            in_code_block: false,
            blockquote_depth: 0,
            code_lang: String::new(),
            code_buf: String::new(),
            list_stack: Vec::new(),
            list_item_index: Vec::new(),
            link_url: None,
            link_text_start: 0,
            table_headers: Vec::new(),
            table_rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: Vec::new(),
            in_table: false,
            in_thead: false,
            col_idx: 0,
        }
    }

    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::Strong) => self.emphasis.bold = true,
            Event::End(TagEnd::Strong) => self.emphasis.bold = false,
            Event::Start(Tag::Emphasis) => self.emphasis.italic = true,
            Event::End(TagEnd::Emphasis) => self.emphasis.italic = false,
            Event::Start(Tag::Strikethrough) => self.emphasis.strikethrough = true,
            Event::End(TagEnd::Strikethrough) => self.emphasis.strikethrough = false,
            Event::Start(Tag::Heading { .. }) => self.flush(),
            Event::End(TagEnd::Heading(level)) => self.handle_heading_end(level),
            Event::End(TagEnd::Paragraph) => self.handle_paragraph_end(),
            Event::Start(Tag::BlockQuote(_)) => {
                self.flush();
                self.blockquote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => self.handle_blockquote_end(),
            Event::Start(Tag::List(start)) => {
                self.list_stack.push(start);
                self.list_item_index.push(start.unwrap_or(1));
            }
            Event::End(TagEnd::List(_)) => self.handle_list_end(),
            Event::Start(Tag::Item) => self.handle_item_start(),
            Event::Start(Tag::CodeBlock(kind)) => self.handle_code_block_start(&kind),
            Event::End(TagEnd::CodeBlock) => self.handle_code_block_end(),
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.link_url = Some(dest_url.to_string());
                self.link_text_start = self.active_spans().len();
            }
            Event::End(TagEnd::Link) => self.handle_link_end(),
            Event::Start(Tag::Table(alignments)) => self.handle_table_start(alignments.len()),
            Event::End(TagEnd::Table) => self.handle_table_end(),
            Event::Start(Tag::TableHead) => self.in_thead = true,
            Event::End(TagEnd::TableHead) => self.in_thead = false,
            Event::Start(Tag::TableRow) => {
                if !self.in_thead {
                    self.current_row = Vec::new();
                }
                self.col_idx = 0;
            }
            Event::End(TagEnd::TableRow) if !self.in_thead => {
                self.table_rows.push(std::mem::take(&mut self.current_row));
            }
            Event::Start(Tag::TableCell) => self.current_cell = Vec::new(),
            Event::End(TagEnd::TableCell) => self.handle_table_cell_end(),
            Event::Code(text) => {
                let span = Span::styled(text.to_string(), self.theme.inline_code());
                self.push_span(span);
            }
            Event::Text(text) => self.handle_text(&text),
            Event::SoftBreak if !self.in_table => self.current_spans.push(Span::raw(" ")),
            Event::End(TagEnd::Item) | Event::HardBreak => self.flush_quote_or_line(),
            Event::Rule => self.handle_rule(),
            _ => {}
        }
    }

    fn flush(&mut self) {
        flush_line(&mut self.current_spans, &mut self.lines);
    }

    fn flush_quote_or_line(&mut self) {
        if self.blockquote_depth > 0 {
            flush_line_blockquote(
                &mut self.current_spans,
                &mut self.lines,
                self.theme,
                self.blockquote_depth,
            );
        } else {
            flush_line(&mut self.current_spans, &mut self.lines);
        }
    }

    fn push_span(&mut self, span: Span<'static>) {
        if self.in_table {
            self.current_cell.push(span);
        } else {
            self.current_spans.push(span);
        }
    }

    fn handle_heading_end(&mut self, level: HeadingLevel) {
        self.flush();
        let style = heading_style(level, self.theme);
        if let Some(last) = self.lines.last_mut() {
            let mut new_spans = Vec::new();
            for span in last.spans.drain(..) {
                new_spans.push(Span::styled(span.content, style));
            }
            *last = Line::from(new_spans);
        }
        self.lines.push(Line::default());
    }

    fn handle_paragraph_end(&mut self) {
        self.flush_quote_or_line();
        if self.blockquote_depth == 0 {
            ensure_blank_gap(&mut self.lines);
        }
    }

    fn handle_blockquote_end(&mut self) {
        self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
        if self.blockquote_depth == 0 {
            ensure_blank_gap(&mut self.lines);
        }
    }

    fn handle_list_end(&mut self) {
        self.list_stack.pop();
        self.list_item_index.pop();
        if self.list_stack.is_empty() {
            ensure_blank_gap(&mut self.lines);
        }
    }

    fn handle_item_start(&mut self) {
        self.flush_quote_or_line();
        let bullet = next_bullet(&self.list_stack, &mut self.list_item_index);
        self.current_spans
            .push(Span::styled(bullet, self.theme.base()));
    }

    fn handle_code_block_start(&mut self, kind: &CodeBlockKind<'_>) {
        self.in_code_block = true;
        self.code_lang = match kind {
            CodeBlockKind::Fenced(lang) => lang.to_string(),
            CodeBlockKind::Indented => String::new(),
        };
        self.code_buf.clear();
        self.flush();
        ensure_blank_gap(&mut self.lines);
    }

    fn handle_code_block_end(&mut self) {
        self.in_code_block = false;
        let highlighted = self.hl.highlight(
            &self.code_lang,
            self.code_buf.trim_end_matches('\n'),
            self.theme,
        );
        self.lines.extend(highlighted);
        ensure_blank_gap(&mut self.lines);
        self.code_lang.clear();
    }

    fn active_spans(&mut self) -> &mut Vec<Span<'static>> {
        if self.in_table {
            &mut self.current_cell
        } else {
            &mut self.current_spans
        }
    }

    fn handle_link_end(&mut self) {
        if let Some(url) = self.link_url.take() {
            let accent = self.theme.accent_color();
            let muted = self.theme.muted();
            let in_table = self.in_table;
            let start = self.link_text_start;
            let spans = self.active_spans();
            let text: String = spans[start..].iter().map(|s| s.content.as_ref()).collect();
            let autolink = !url.is_empty() && url == text;
            for span in &mut spans[start..] {
                span.style = span.style.fg(accent);
                if autolink {
                    span.style = span.style.add_modifier(Modifier::UNDERLINED);
                }
            }
            if !in_table && !url.is_empty() && url != text {
                spans.push(Span::styled(
                    format!(" ({url})"),
                    muted.add_modifier(Modifier::UNDERLINED),
                ));
            }
        }
        self.link_text_start = 0;
    }

    fn handle_table_start(&mut self, cols: usize) {
        self.in_table = true;
        self.table_headers = vec![Vec::new(); cols];
        self.table_rows.clear();
        self.flush();
    }

    fn handle_table_end(&mut self) {
        self.in_table = false;
        render_table(
            &self.table_headers,
            &self.table_rows,
            self.theme,
            &mut self.lines,
        );
        ensure_blank_gap(&mut self.lines);
        self.table_headers.clear();
        self.table_rows.clear();
    }

    fn handle_table_cell_end(&mut self) {
        if self.in_thead {
            if self.col_idx < self.table_headers.len() {
                self.table_headers[self.col_idx].clone_from(&self.current_cell);
            }
        } else {
            self.current_row.push(self.current_cell.clone());
        }
        self.current_cell.clear();
        self.col_idx += 1;
    }

    fn handle_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_buf.push_str(text);
            return;
        }
        let style = self.emphasis.style(self.theme);
        for (i, segment) in text.split('\n').enumerate() {
            if i > 0 {
                self.flush_quote_or_line();
            }
            if !segment.is_empty() {
                self.push_span(Span::styled(segment.to_string(), style));
            }
        }
    }

    fn handle_rule(&mut self) {
        self.flush();
        self.lines.push(Line::from(Span::styled(
            symbols::ui::HRULE,
            self.theme.muted(),
        )));
        self.lines.push(Line::default());
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        while self.lines.last().is_some_and(|l| l.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

fn heading_style(level: HeadingLevel, theme: Theme) -> ratatui::style::Style {
    match level {
        HeadingLevel::H1 => theme.accent().add_modifier(Modifier::BOLD),
        HeadingLevel::H2 => theme.accent(),
        HeadingLevel::H3 => theme.key(),
        HeadingLevel::H4 => theme.base().add_modifier(Modifier::BOLD),
        _ => theme.muted().add_modifier(Modifier::BOLD),
    }
}

fn flush_line(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

fn flush_line_blockquote(
    spans: &mut Vec<Span<'static>>,
    lines: &mut Vec<Line<'static>>,
    theme: Theme,
    depth: usize,
) {
    if spans.is_empty() {
        return;
    }
    let gutter = symbols::ui::QUOTE_GUTTER.repeat(depth);
    let mut row_spans = vec![Span::styled(gutter, theme.muted())];
    row_spans.extend(std::mem::take(spans));
    lines.push(Line::from(row_spans));
}

fn ensure_blank_gap(lines: &mut Vec<Line<'static>>) {
    if lines.last().is_some_and(|l| !l.spans.is_empty()) {
        lines.push(Line::default());
    }
}

fn render_table(
    headers: &[Vec<Span<'static>>],
    rows: &[Vec<Vec<Span<'static>>>],
    theme: Theme,
    lines: &mut Vec<Line<'static>>,
) {
    if headers.is_empty() {
        return;
    }
    let col_count = headers.len();

    let col_widths: Vec<usize> = (0..col_count)
        .map(|col| {
            let header_w = span_text_width(&headers[col]);
            let max_row_w = rows
                .iter()
                .filter_map(|row| row.get(col))
                .map(|cell| span_text_width(cell))
                .max()
                .unwrap_or(0);
            header_w.max(max_row_w).max(1)
        })
        .collect();

    lines.push(build_table_row(headers.iter(), &col_widths, theme, true));

    let sep: String = col_widths
        .iter()
        .map(|&w| "─".repeat(w))
        .collect::<Vec<_>>()
        .join("─┼─");
    lines.push(Line::from(Span::styled(sep, theme.muted())));

    for row in rows {
        lines.push(build_table_row(row.iter(), &col_widths, theme, false));
    }
}

fn build_table_row<'a, I>(
    cells: I,
    col_widths: &[usize],
    theme: Theme,
    header: bool,
) -> Line<'static>
where
    I: Iterator<Item = &'a Vec<Span<'static>>>,
{
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (col, cell) in cells.enumerate() {
        if col > 0 {
            spans.push(Span::styled(" │ ", theme.muted()));
        }
        let w = col_widths.get(col).copied().unwrap_or(1);
        let text_w = span_text_width(cell);
        let pad = w.saturating_sub(text_w);
        for s in cell {
            let style = if header {
                s.style.add_modifier(Modifier::BOLD)
            } else {
                s.style
            };
            spans.push(Span::styled(s.content.clone(), style));
        }
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
    }
    Line::from(spans)
}

fn span_text_width(spans: &[Span<'static>]) -> usize {
    use unicode_width::UnicodeWidthStr;
    spans.iter().map(|s| s.content.width()).sum()
}

fn next_bullet(list_stack: &[Option<u64>], list_item_index: &mut [u64]) -> String {
    let depth = list_stack.len().saturating_sub(1);
    let indent = "  ".repeat(depth);
    let marker = match list_stack.last() {
        Some(Some(_)) => {
            let idx = list_item_index.last_mut().map_or(1, |i| {
                let v = *i;
                *i += 1;
                v
            });
            format!("{idx}. ")
        }
        Some(None) | None => symbols::ui::BULLET.to_owned(),
    };
    format!("{indent}{marker}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::PlainHighlighter;
    use crate::theme::Theme;

    fn render_plain(md: &str) -> Vec<Line<'static>> {
        render(md, Theme::dark(), &PlainHighlighter)
    }

    #[test]
    fn bold_text_has_bold_modifier() {
        let lines = render_plain("**hello**");
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn strikethrough_has_crossed_out() {
        let lines = render_plain("~~old~~");
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT))
        );
    }

    #[test]
    fn inline_code_uses_accent() {
        let theme = Theme::dark();
        let lines = render("use `foo` here", theme, &PlainHighlighter);
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        let code_span = spans
            .iter()
            .find(|s| s.content.as_ref() == "foo")
            .expect("inline code span without padding");
        assert_eq!(code_span.style.fg, Some(theme.accent_color()));
        assert_eq!(code_span.style.bg, None);
    }

    #[test]
    fn code_block_renders_content() {
        let lines = render_plain("```rust\nfn main() {}\n```");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("fn main() {}"));
        assert!(!text.contains('│'));
        assert!(!text.contains("rust"));
    }

    #[test]
    fn code_block_preceded_by_blank_line() {
        let lines = render_plain("before\n```rust\nfn x() {}\n```");
        let code_idx = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content.contains("fn x")))
            .expect("code line");
        assert!(lines[code_idx - 1].spans.is_empty());
        assert!(!lines[code_idx - 2].spans.is_empty());
    }

    #[test]
    fn unordered_list_renders_bullets() {
        let theme = Theme::dark();
        let lines = render("- one\n- two", theme, &PlainHighlighter);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("- one"));
        assert!(!text.contains('•'));
        let bullet = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "- ")
            .expect("bullet span");
        assert_eq!(bullet.style.fg, Some(theme.fg_color()));
    }

    #[test]
    fn list_followed_by_paragraph_has_gap() {
        let lines = render_plain("- one\n- two\n\nafter");
        let after_idx = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content.contains("after")))
            .expect("after line");
        assert!(lines[after_idx - 1].spans.is_empty());
        assert!(!lines[after_idx - 2].spans.is_empty());
    }

    #[test]
    fn ordered_list_renders_numbers() {
        let lines = render_plain("1. first\n2. second");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("1.") || text.contains("2."));
    }

    #[test]
    fn heading_h1_uses_bold_accent_no_hash() {
        let theme = Theme::dark();
        let lines = render("# Title", theme, &PlainHighlighter);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!text.contains('#'));
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(spans.iter().any(|s| {
            s.style.add_modifier.contains(Modifier::BOLD)
                && s.style.fg == Some(theme.accent_color())
        }));
    }

    #[test]
    fn heading_h2_uses_accent_no_hash() {
        let theme = Theme::dark();
        let lines = render("## Section", theme, &PlainHighlighter);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!text.contains("##"));
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.fg == Some(theme.accent_color()))
        );
    }

    #[test]
    fn horizontal_rule_emits_rule_glyph() {
        let lines = render_plain("---");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains(crate::symbols::ui::HRULE));
    }

    #[test]
    fn blockquote_content_line_has_gutter() {
        let theme = Theme::dark();
        let lines = render("> quoted text", theme, &PlainHighlighter);
        let line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("quoted")))
            .expect("quoted content line");
        assert!(line.spans.first().is_some_and(|s| s.content.contains('▎')));
        let content = line
            .spans
            .iter()
            .find(|s| s.content.contains("quoted"))
            .expect("content span");
        assert_eq!(content.style.fg, Some(theme.fg_color()));
    }

    #[test]
    fn nested_blockquote_repeats_gutter() {
        let lines = render_plain("> outer\n>> nested");
        let line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("nested")))
            .expect("nested content line");
        let gutter = line.spans.first().expect("gutter span");
        assert_eq!(gutter.content.matches('▎').count(), 2);
    }

    #[test]
    fn link_text_accent_with_url_text() {
        let theme = Theme::dark();
        let lines = render("[docs](https://example.com)", theme, &PlainHighlighter);
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("docs"));
        assert!(text.contains("example.com"));
        let link = spans
            .iter()
            .find(|s| s.content.contains("docs"))
            .expect("link span");
        assert_eq!(link.style.fg, Some(theme.accent_color()));
        assert!(!link.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn link_inside_table_cell_is_accented() {
        let theme = Theme::dark();
        let table = "| name | link |\n| --- | --- |\n| a | [docs](https://example.com) |";
        let lines = render(table, theme, &PlainHighlighter);
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("docs"));
        assert!(!text.contains("example.com"));
        let link = spans
            .iter()
            .find(|s| s.content.contains("docs"))
            .expect("table link span");
        assert_eq!(link.style.fg, Some(theme.accent_color()));
    }

    #[test]
    fn italic_text_has_italic_modifier() {
        let lines = render_plain("*hello*");
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::ITALIC))
        );
    }

    #[test]
    fn table_renders_header_bold() {
        let lines = render_plain("| a | b |\n|---|---|\n| 1 | 2 |");
        let spans: Vec<_> = lines.iter().flat_map(|l| &l.spans).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn table_separator_aligns_with_cells() {
        use unicode_width::UnicodeWidthStr;
        let lines = render_plain("| name | x |\n|---|---|\n| alice | 1 |");
        let widths: Vec<usize> = lines
            .iter()
            .filter(|l| !l.spans.is_empty())
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref().width())
                    .sum::<usize>()
            })
            .collect();
        assert_eq!(widths.len(), 3);
        assert!(widths.iter().all(|w| *w == widths[0]));
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains('│'));
        assert!(text.contains('┼'));
    }

    #[test]
    fn plain_text_passes_through() {
        let lines = render_plain("hello world");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("hello world"));
    }
}

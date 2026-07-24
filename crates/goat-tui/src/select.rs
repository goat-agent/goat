use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Selection {
    pub anchor: (usize, u16),
    pub focus: (usize, u16),
    pub dragging: bool,
}

impl Selection {
    pub(crate) fn new(pos: (usize, u16)) -> Self {
        Self {
            anchor: pos,
            focus: pos,
            dragging: true,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    pub(crate) fn bounds(&self) -> ((usize, u16), (usize, u16)) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

fn gutter_width(line: &Line<'_>) -> u16 {
    line.spans.first().map_or(0, |span| {
        let cols: usize = span
            .content
            .chars()
            .filter_map(UnicodeWidthChar::width)
            .sum();
        u16::try_from(cols).unwrap_or(u16::MAX)
    })
}

fn line_slice(line: &Line<'_>, col_start: u16, col_end: u16) -> String {
    let skip = gutter_width(line);
    let start = col_start.max(skip);
    let mut col: u16 = 0;
    let mut out = String::new();
    for (i, span) in line.spans.iter().enumerate() {
        for ch in span.content.chars() {
            let w = u16::try_from(ch.width().unwrap_or(0)).unwrap_or(0);
            if i > 0 && col >= start && col < col_end {
                out.push(ch);
            }
            col = col.saturating_add(w);
        }
    }
    out.trim_end().to_string()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn line_cells(line: &Line<'_>) -> Vec<(u16, u16, char)> {
    let mut cells: Vec<(u16, u16, char)> = Vec::new();
    let mut c: u16 = 0;
    for (i, span) in line.spans.iter().enumerate() {
        for ch in span.content.chars() {
            let w = u16::try_from(ch.width().unwrap_or(0)).unwrap_or(0);
            if i > 0 {
                cells.push((c, w, ch));
            }
            c = c.saturating_add(w);
        }
    }
    cells
}

fn cell_at(cells: &[(u16, u16, char)], target: u16) -> Option<usize> {
    cells
        .iter()
        .position(|(sc, w, _)| *sc <= target && target < sc.saturating_add((*w).max(1)))
}

pub(crate) fn word_bounds(line: &Line<'_>, col: u16) -> Option<(u16, u16)> {
    let cells = line_cells(line);
    let pos = cell_at(&cells, col.max(gutter_width(line)))?;
    if !is_word(cells[pos].2) {
        return None;
    }
    let mut lo = pos;
    while lo > 0 && is_word(cells[lo - 1].2) {
        lo -= 1;
    }
    let mut hi = pos;
    while hi + 1 < cells.len() && is_word(cells[hi + 1].2) {
        hi += 1;
    }
    Some((cells[lo].0, cells[hi].0.saturating_add(cells[hi].1)))
}

fn prefix_at(chars: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(k, pc)| chars.get(i + k) == Some(&pc))
}

fn url_scheme_at(chars: &[char], i: usize) -> bool {
    let boundary = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(';
    boundary && (prefix_at(chars, i, "https://") || prefix_at(chars, i, "http://"))
}

fn url_trailing(c: char) -> bool {
    matches!(
        c,
        ')' | ']' | '}' | '.' | ',' | ';' | '!' | '?' | '"' | '\''
    )
}

pub(crate) fn url_at(line: &Line<'_>, col: u16) -> Option<String> {
    let cells = line_cells(line);
    let pos = cell_at(&cells, col.max(gutter_width(line)))?;
    let chars: Vec<char> = cells.iter().map(|(_, _, ch)| *ch).collect();
    let mut i = 0;
    while i < chars.len() {
        if url_scheme_at(&chars, i) {
            let mut end = i;
            while end < chars.len() && !chars[end].is_whitespace() {
                end += 1;
            }
            while end > i && url_trailing(chars[end - 1]) {
                end -= 1;
            }
            if (i..end).contains(&pos) {
                return Some(chars[i..end].iter().collect());
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

pub(crate) fn extract_split(
    head: &[Line<'_>],
    tail: &[Line<'_>],
    anchor: (usize, u16),
    focus: (usize, u16),
) -> String {
    let (start, end) = if anchor <= focus {
        (anchor, focus)
    } else {
        (focus, anchor)
    };
    let at = |idx: usize| {
        if idx < head.len() {
            head.get(idx)
        } else {
            tail.get(idx - head.len())
        }
    };
    let mut rows: Vec<String> = Vec::new();
    for idx in start.0..=end.0 {
        let Some(line) = at(idx) else {
            continue;
        };
        let col_start = if idx == start.0 { start.1 } else { 0 };
        let col_end = if idx == end.0 { end.1 } else { u16::MAX };
        rows.push(line_slice(line, col_start, col_end));
    }
    rows.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{Selection, extract_split};
    use ratatui::text::{Line, Span};

    fn line(text: &str) -> Line<'static> {
        Line::from(vec![Span::raw("● "), Span::raw(text.to_owned())])
    }

    #[test]
    fn extract_strips_gutter_single_line() {
        let lines = vec![line("hello world")];
        let got = extract_split(&lines, &[], (0, 0), (0, u16::MAX));
        assert_eq!(got, "hello world");
    }

    #[test]
    fn extract_partial_columns_within_line() {
        let lines = vec![line("hello world")];
        let got = extract_split(&lines, &[], (0, 2), (0, 7));
        assert_eq!(got, "hello");
    }

    #[test]
    fn extract_multi_line_joins_with_newline() {
        let lines = vec![line("first"), line("second")];
        let got = extract_split(&lines, &[], (0, 0), (1, u16::MAX));
        assert_eq!(got, "first\nsecond");
    }

    #[test]
    fn extract_partial_first_line_to_partial_last() {
        let lines = vec![line("hello"), line("world")];
        let got = extract_split(&lines, &[], (0, 4), (1, 5));
        assert_eq!(got, "llo\nwor");
    }

    #[test]
    fn extract_reversed_bounds_normalized() {
        let lines = vec![line("abc")];
        let got = extract_split(&lines, &[], (0, u16::MAX), (0, 0));
        assert_eq!(got, "abc");
    }

    #[test]
    fn extract_handles_blank_line_without_panic() {
        let lines = vec![line("a"), Line::default(), line("b")];
        let got = extract_split(&lines, &[], (0, 0), (2, u16::MAX));
        assert_eq!(got, "a\n\nb");
    }

    #[test]
    fn extract_maps_wide_chars_by_display_column() {
        let lines = vec![Line::from(vec![Span::raw("● "), Span::raw("한글x")])];
        let got = extract_split(&lines, &[], (0, 2), (0, 4));
        assert_eq!(got, "한");
    }

    #[test]
    fn extract_split_spans_head_and_tail() {
        let head = vec![line("static one")];
        let tail = vec![line("live two")];
        let got = extract_split(&head, &tail, (0, 0), (1, u16::MAX));
        assert_eq!(got, "static one\nlive two");
    }

    #[test]
    fn word_bounds_selects_word_at_column() {
        use super::word_bounds;
        let l = line("foo bar_baz qux");
        assert_eq!(word_bounds(&l, 3), Some((2, 5)));
        assert_eq!(word_bounds(&l, 8), Some((6, 13)));
        assert_eq!(word_bounds(&l, 5), None);
    }

    #[test]
    fn url_at_finds_autolink_and_ignores_words() {
        use super::url_at;
        let l = line("see https://example.com/x now");
        assert_eq!(url_at(&l, 10).as_deref(), Some("https://example.com/x"));
        assert_eq!(url_at(&l, 2), None);
    }

    #[test]
    fn url_at_trims_wrapping_paren() {
        use super::url_at;
        let l = line("docs (https://example.com)");
        assert_eq!(url_at(&l, 12).as_deref(), Some("https://example.com"));
    }

    #[test]
    fn url_at_off_url_returns_none() {
        use super::url_at;
        let l = line("no link here at all");
        assert_eq!(url_at(&l, 8), None);
    }

    #[test]
    fn selection_bounds_normalizes_order() {
        let sel = Selection {
            anchor: (2, 3),
            focus: (0, 1),
            dragging: false,
        };
        assert_eq!(sel.bounds(), ((0, 1), (2, 3)));
    }
}

use std::ops::Range;

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    width: usize,
    style: Style,
}

pub(crate) fn wrap_line(line: &Line<'_>, width: u16) -> Vec<Line<'static>> {
    let cells = flatten(line);
    let max = usize::from(width);
    if max == 0 || cells.is_empty() {
        return vec![rebuild(line, &cells)];
    }
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
    let mut row_width = 0usize;
    let mut i = 0;
    while i < cells.len() {
        let gap = cells[i].ch.is_whitespace();
        let mut j = i;
        let mut seg_width = 0usize;
        while j < cells.len() && cells[j].ch.is_whitespace() == gap {
            seg_width += cells[j].width;
            j += 1;
        }
        if row_width + seg_width <= max {
            row.extend_from_slice(&cells[i..j]);
            row_width += seg_width;
        } else if gap {
            flush(&mut rows, line, &mut row, &mut row_width);
        } else if seg_width <= max {
            flush(&mut rows, line, &mut row, &mut row_width);
            row.extend_from_slice(&cells[i..j]);
            row_width = seg_width;
        } else {
            for cell in &cells[i..j] {
                if row_width + cell.width > max && row_width > 0 {
                    flush(&mut rows, line, &mut row, &mut row_width);
                }
                row_width += cell.width;
                row.push(*cell);
            }
        }
        i = j;
    }
    trim_trailing(&mut row);
    if !row.is_empty() || rows.is_empty() {
        rows.push(rebuild(line, &row));
    }
    rows
}

pub(crate) fn wrap_widths(widths: &[usize], width: u16) -> Vec<Range<usize>> {
    let max = usize::from(width);
    let mut rows = Vec::new();
    if max == 0 {
        rows.push(0..widths.len());
        return rows;
    }
    let mut start = 0usize;
    let mut row_width = 0usize;
    for (i, &cw) in widths.iter().enumerate() {
        if row_width + cw > max && row_width > 0 {
            rows.push(start..i);
            start = i;
            row_width = 0;
        }
        row_width += cw;
    }
    rows.push(start..widths.len());
    rows
}

fn flatten(line: &Line<'_>) -> Vec<Cell> {
    line.spans
        .iter()
        .flat_map(|span| {
            let style = span.style;
            span.content.chars().map(move |ch| Cell {
                ch,
                width: ch.width().unwrap_or(0),
                style,
            })
        })
        .collect()
}

fn flush(
    rows: &mut Vec<Line<'static>>,
    template: &Line<'_>,
    row: &mut Vec<Cell>,
    width: &mut usize,
) {
    trim_trailing(row);
    if !row.is_empty() {
        rows.push(rebuild(template, row));
        row.clear();
    }
    *width = 0;
}

fn trim_trailing(row: &mut Vec<Cell>) {
    while row.last().is_some_and(|cell| cell.ch.is_whitespace()) {
        row.pop();
    }
}

fn rebuild(template: &Line<'_>, cells: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut style: Option<Style> = None;
    for cell in cells {
        match style {
            Some(s) if s == cell.style => buf.push(cell.ch),
            Some(s) => {
                spans.push(Span::styled(std::mem::take(&mut buf), s));
                buf.push(cell.ch);
                style = Some(cell.style);
            }
            None => {
                buf.push(cell.ch);
                style = Some(cell.style);
            }
        }
    }
    if let Some(s) = style {
        spans.push(Span::styled(buf, s));
    }
    let mut out = Line::from(spans);
    out.style = template.style;
    out.alignment = template.alignment;
    out
}

#[cfg(test)]
mod tests {
    use ratatui::{
        style::{Color, Style},
        text::{Line, Span},
    };

    use super::{wrap_line, wrap_widths};

    fn text(line: &Line<'_>) -> Vec<String> {
        vec![line.spans.iter().map(|s| s.content.as_ref()).collect()]
    }

    fn rows(input: &str, width: u16) -> Vec<String> {
        wrap_line(&Line::from(input.to_owned()), width)
            .iter()
            .flat_map(text)
            .collect()
    }

    #[test]
    fn word_wrap_breaks_at_word_boundaries() {
        assert_eq!(
            rows("aaaaaaa bbbbbbb ccccccc", 12),
            vec!["aaaaaaa", "bbbbbbb", "ccccccc"]
        );
    }

    #[test]
    fn oversized_word_hard_splits() {
        assert_eq!(rows("abcdefghij", 5), vec!["abcde", "fghij"]);
    }

    #[test]
    fn wide_char_moves_whole_to_next_row() {
        assert_eq!(rows("ab한글", 5), vec!["ab한", "글"]);
    }

    #[test]
    fn gap_at_break_is_dropped() {
        assert_eq!(rows("aa bb cc", 5), vec!["aa bb", "cc"]);
    }

    #[test]
    fn leading_indent_is_preserved() {
        assert_eq!(rows("  code", 10), vec!["  code"]);
    }

    #[test]
    fn empty_line_yields_one_row() {
        assert_eq!(wrap_line(&Line::default(), 10).len(), 1);
    }

    #[test]
    fn width_zero_yields_one_row() {
        assert_eq!(wrap_line(&Line::from("abc"), 0).len(), 1);
    }

    #[test]
    fn styles_survive_wrapping() {
        let red = Style::new().fg(Color::Red);
        let blue = Style::new().fg(Color::Blue);
        let line = Line::from(vec![Span::styled("aaaa ", red), Span::styled("bbbb", blue)]);
        let wrapped = wrap_line(&line, 5);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].spans[0].style, red);
        assert_eq!(wrapped[1].spans[0].style, blue);
        assert_eq!(wrapped[1].spans[0].content.as_ref(), "bbbb");
    }

    #[test]
    fn every_row_fits_width() {
        use unicode_width::UnicodeWidthStr;
        let wrapped = wrap_line(&Line::from("한glyph 글자 mixed width 텍스트입니다"), 9);
        for row in &wrapped {
            let w: usize = row.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert!(w <= 9);
        }
    }

    #[test]
    fn wrap_widths_handles_wide_boundary() {
        assert_eq!(wrap_widths(&[2, 2, 2], 4), vec![0..2, 2..3]);
    }

    #[test]
    fn wrap_widths_empty_yields_one_row() {
        assert_eq!(wrap_widths(&[], 8), vec![0..0]);
    }

    #[test]
    fn wrap_widths_keeps_atomic_unit_whole() {
        assert_eq!(wrap_widths(&[1, 9, 1], 6), vec![0..1, 1..2, 2..3]);
    }
}

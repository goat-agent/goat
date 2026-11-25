use std::fmt::Write;

use serde::Deserialize;

const CONTEXT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Anchor {
    line: usize,
    hash: u16,
}

impl Anchor {
    fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.split('|').next().unwrap_or(raw).trim();
        let invalid = || format!("anchor `{raw}` is not in the form N:hhh");
        let (line, hash) = raw.split_once(':').ok_or_else(invalid)?;
        let line = line.trim().parse().map_err(|_| invalid())?;
        let hash = u16::from_str_radix(hash.trim(), 16).map_err(|_| invalid())?;
        if line == 0 {
            return Err(invalid());
        }
        Ok(Self { line, hash })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OpKind {
    Replace,
    InsertAfter,
    Delete,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RawOp {
    op: OpKind,
    from: Option<String>,
    to: Option<String>,
    lines: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Replace {
        from: Anchor,
        to: Anchor,
        lines: Vec<String>,
    },
    Insert {
        after: Option<Anchor>,
        lines: Vec<String>,
    },
}

impl Op {
    fn parse(raw: RawOp) -> Result<Self, String> {
        let anchor = |raw: Option<&String>| raw.map(|a| Anchor::parse(a)).transpose();
        let from = anchor(raw.from.as_ref())?;
        let to = anchor(raw.to.as_ref())?;
        let missing_from = || format!("{:?} needs `from`", raw.op);
        match raw.op {
            OpKind::Replace | OpKind::Delete => {
                let from = from.ok_or_else(missing_from)?;
                let to = to.unwrap_or(from);
                if to.line < from.line {
                    return Err(format!("range {} to {} is reversed", from.line, to.line));
                }
                let lines = match raw.op {
                    OpKind::Delete => Vec::new(),
                    _ => raw.lines.ok_or("replace needs `lines`")?,
                };
                Ok(Self::Replace { from, to, lines })
            }
            OpKind::InsertAfter => Ok(Self::Insert {
                after: from,
                lines: raw.lines.ok_or("insert_after needs `lines`")?,
            }),
        }
    }

    fn start(&self) -> usize {
        match self {
            Self::Replace { from, .. } => from.line * 2,
            Self::Insert { after, .. } => after.map_or(0, |a| a.line) * 2 + 1,
        }
    }

    fn end(&self) -> usize {
        match self {
            Self::Replace { to, .. } => to.line * 2,
            Self::Insert { .. } => self.start(),
        }
    }

    fn anchors(&self) -> impl Iterator<Item = Anchor> {
        let (a, b) = match self {
            Self::Replace { from, to, .. } => (Some(*from), Some(*to)),
            Self::Insert { after, .. } => (*after, None),
        };
        a.into_iter().chain(b)
    }
}

#[derive(Debug)]
pub(crate) struct Document {
    lines: Vec<String>,
    crlf: bool,
    trailing_newline: bool,
}

impl Document {
    pub(crate) fn parse(text: &str) -> Self {
        let crlf = text.contains("\r\n");
        let trailing_newline = text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(text);
        let lines = if text.is_empty() {
            Vec::new()
        } else {
            body.split('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
                .collect()
        };
        Self {
            lines,
            crlf,
            trailing_newline,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn render(&self) -> String {
        let newline = if self.crlf { "\r\n" } else { "\n" };
        let mut text = self.lines.join(newline);
        if self.trailing_newline && !self.lines.is_empty() {
            text.push_str(newline);
        }
        text
    }

    pub(crate) fn tagged(&self, first: usize, last: usize) -> String {
        let mut out = String::new();
        for (index, line) in self
            .lines
            .iter()
            .enumerate()
            .take(last)
            .skip(first.saturating_sub(1))
        {
            writeln!(out, "{}", tag_line(index + 1, line)).ok();
        }
        out
    }

    pub(crate) fn apply(&mut self, raw: Vec<RawOp>) -> Result<String, String> {
        let mut ops = raw
            .into_iter()
            .map(Op::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if ops.is_empty() {
            return Err("no edits given".to_owned());
        }
        self.check_anchors(&ops)?;
        ops.sort_by_key(Op::start);
        for pair in ops.windows(2) {
            if pair[1].start() <= pair[0].end() {
                return Err("edits overlap; merge them into one edit".to_owned());
            }
        }

        let mut regions = Vec::with_capacity(ops.len());
        let mut shift = 0isize;
        for op in &ops {
            let (first, removed, added) = match op {
                Op::Replace { from, to, lines } => {
                    (from.line, to.line - from.line + 1, lines.len())
                }
                Op::Insert { after, lines } => (after.map_or(0, |a| a.line) + 1, 0, lines.len()),
            };
            let new_first = first.saturating_add_signed(shift);
            regions.push((new_first, added));
            shift += isize::try_from(added).unwrap_or(isize::MAX)
                - isize::try_from(removed).unwrap_or(isize::MAX);
        }
        for op in ops.into_iter().rev() {
            match op {
                Op::Replace { from, to, lines } => {
                    self.lines.splice(from.line - 1..to.line, lines);
                }
                Op::Insert { after, lines } => {
                    let at = after.map_or(0, |a| a.line);
                    self.lines.splice(at..at, lines);
                }
            }
        }

        let mut preview = String::new();
        for (first, added) in regions {
            let start = first.saturating_sub(CONTEXT).max(1);
            let end = (first + added + CONTEXT).min(self.lines.len());
            if start <= end {
                preview.push_str(&self.tagged(start, end));
                preview.push_str("...\n");
            }
        }
        Ok(preview)
    }

    fn check_anchors(&self, ops: &[Op]) -> Result<(), String> {
        let mut problems = String::new();
        for anchor in ops.iter().flat_map(Op::anchors) {
            match self.lines.get(anchor.line - 1) {
                None => {
                    writeln!(
                        problems,
                        "line {} does not exist; the file has {} lines",
                        anchor.line,
                        self.lines.len()
                    )
                    .ok();
                }
                Some(line) if hash(line) != anchor.hash => {
                    writeln!(
                        problems,
                        "line {} changed since it was read; current lines:",
                        anchor.line
                    )
                    .ok();
                    let start = anchor.line.saturating_sub(CONTEXT).max(1);
                    problems.push_str(&self.tagged(start, anchor.line + CONTEXT));
                }
                Some(_) => {}
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(format!("edit rejected, nothing was changed:\n{problems}"))
        }
    }
}

pub(crate) fn locate(raw: &str, lines: &[&str]) -> Result<usize, String> {
    if let Ok(number) = raw.trim().parse::<usize>() {
        return match number {
            1.. if number <= lines.len() => Ok(number),
            _ => Err(format!("line {number} is past the end of the file")),
        };
    }
    let anchor = Anchor::parse(raw)?;
    let text = lines
        .get(anchor.line - 1)
        .ok_or_else(|| format!("line {} is past the end of the file", anchor.line))?;
    if hash(text) != anchor.hash {
        return Err(format!(
            "line {} changed since it was read; read it again",
            anchor.line
        ));
    }
    Ok(anchor.line)
}

pub(crate) fn tag_line(number: usize, line: &str) -> String {
    format!("{number}:{:03x}|{line}", hash(line))
}

fn hash(line: &str) -> u16 {
    let mut state: u32 = 0x811c_9dc5;
    for byte in line.bytes() {
        state ^= u32::from(byte);
        state = state.wrapping_mul(0x0100_0193);
    }
    u16::try_from((state ^ (state >> 16)) & 0x0fff).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_found_by_number_or_by_a_fresh_anchor() {
        let lines = ["fn main() {", "    run();", "}"];
        let anchor = tag_line(2, lines[1]);

        assert_eq!(locate("3", &lines), Ok(3));
        assert_eq!(locate(&anchor, &lines), Ok(2));
        assert_eq!(
            locate(&tag_line(2, "    stop();"), &lines),
            Err("line 2 changed since it was read; read it again".to_owned())
        );
    }

    fn anchor(doc: &Document, line: usize) -> String {
        format!("{line}:{:03x}", hash(&doc.lines[line - 1]))
    }

    fn op(kind: OpKind, from: Option<String>, to: Option<String>, lines: Option<&[&str]>) -> RawOp {
        RawOp {
            op: kind,
            from,
            to,
            lines: lines.map(|l| l.iter().map(|s| (*s).to_owned()).collect()),
        }
    }

    #[test]
    fn replace_one_line() {
        let mut doc = Document::parse("a\nb\nc\n");
        let from = anchor(&doc, 2);
        doc.apply(vec![op(OpKind::Replace, Some(from), None, Some(&["B"]))])
            .unwrap();
        assert_eq!(doc.render(), "a\nB\nc\n");
    }

    #[test]
    fn insert_at_top_and_after_last_line() {
        let mut doc = Document::parse("a\nb");
        let last = anchor(&doc, 2);
        doc.apply(vec![
            op(OpKind::InsertAfter, None, None, Some(&["top"])),
            op(OpKind::InsertAfter, Some(last), None, Some(&["end"])),
        ])
        .unwrap();
        assert_eq!(doc.render(), "top\na\nb\nend");
    }

    #[test]
    fn delete_range_keeps_crlf() {
        let mut doc = Document::parse("a\r\nb\r\nc\r\nd\r\n");
        let (from, to) = (anchor(&doc, 2), anchor(&doc, 3));
        doc.apply(vec![op(OpKind::Delete, Some(from), Some(to), None)])
            .unwrap();
        assert_eq!(doc.render(), "a\r\nd\r\n");
    }

    #[test]
    fn several_edits_apply_against_the_original_numbers() {
        let mut doc = Document::parse("1\n2\n3\n4\n5\n");
        let (two, four) = (anchor(&doc, 2), anchor(&doc, 4));
        doc.apply(vec![
            op(OpKind::Replace, Some(four), None, Some(&["four", "more"])),
            op(OpKind::Delete, Some(two), None, None),
        ])
        .unwrap();
        assert_eq!(doc.render(), "1\n3\nfour\nmore\n5\n");
    }

    #[test]
    fn edit_with_stale_hash_is_rejected() {
        let mut doc = Document::parse("fn main() {}\n");
        let err = doc
            .apply(vec![op(
                OpKind::Replace,
                Some("1:000".to_owned()),
                None,
                Some(&["x"]),
            )])
            .unwrap_err();
        assert!(err.contains("changed since it was read"));
        assert_eq!(doc.render(), "fn main() {}\n");
    }

    #[test]
    fn anchor_copied_with_line_text_is_accepted() {
        let doc = Document::parse("hello\n");
        let full = tag_line(1, "hello");
        assert_eq!(
            Anchor::parse(&full).unwrap(),
            Anchor::parse(&anchor(&doc, 1)).unwrap()
        );
    }

    #[test]
    fn overlapping_edits_are_rejected() {
        let mut doc = Document::parse("a\nb\nc\n");
        let (one, two) = (anchor(&doc, 1), anchor(&doc, 2));
        let err = doc
            .apply(vec![
                op(OpKind::Replace, Some(one.clone()), Some(two), Some(&["x"])),
                op(OpKind::InsertAfter, Some(one), None, Some(&["y"])),
            ])
            .unwrap_err();
        assert!(err.contains("overlap"));
    }

    #[test]
    fn empty_file_accepts_insert_at_top() {
        let mut doc = Document::parse("");
        doc.apply(vec![op(OpKind::InsertAfter, None, None, Some(&["new"]))])
            .unwrap();
        assert_eq!(doc.render(), "new");
    }
}

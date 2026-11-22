use std::mem;

const FENCE: &str = "```";
const MARKERS: [(&str, &str); 4] = [("- ", "• "), ("* ", "• "), ("+ ", "• "), ("> ", "│ ")];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    Plain,
    Strong,
    Code,
    Heading,
    Marker,
    Open,
    Close,
    Block,
}

#[derive(Default)]
pub(crate) struct Markdown {
    pending: String,
    line: Line,
    block: Block,
    strong: bool,
    code: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Line {
    #[default]
    Start,
    Body,
    Heading,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Block {
    #[default]
    Prose,
    Code,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    Streaming,
    Finished,
}

impl Markdown {
    pub(crate) fn push(&mut self, chunk: &str, out: &mut impl FnMut(Style, &str)) {
        self.pending.push_str(chunk);
        self.drain(Input::Streaming, out);
    }

    pub(crate) fn finish(&mut self, out: &mut impl FnMut(Style, &str)) {
        self.drain(Input::Finished, out);
        *self = Self::default();
    }

    fn drain(&mut self, input: Input, out: &mut impl FnMut(Style, &str)) {
        let mut pending = mem::take(&mut self.pending);
        let mut at = 0;
        while at < pending.len() {
            let rest = &pending[at..];
            let used = match self.line {
                Line::Start => self.line_start(rest, input, out),
                Line::Body | Line::Heading => self.inline(rest, input, out),
            };
            let Some(used) = used else {
                break;
            };
            at += used;
        }
        pending.drain(..at);
        self.pending = pending;
    }

    fn line_start(
        &mut self,
        rest: &str,
        input: Input,
        out: &mut impl FnMut(Style, &str),
    ) -> Option<usize> {
        let end = rest.find('\n');
        let is_complete = input == Input::Finished || end.is_some();
        let head = &rest[..end.unwrap_or(rest.len())];
        let body = head.trim_start_matches(' ');
        let indent = head.len() - body.len();
        if body.starts_with(FENCE) {
            if !is_complete {
                return None;
            }
            match self.block {
                Block::Prose => {
                    let info = body.trim_start_matches('`');
                    out(
                        Style::Open,
                        info.split_whitespace().next().unwrap_or_default(),
                    );
                    self.block = Block::Code;
                }
                Block::Code => {
                    out(Style::Close, "");
                    self.block = Block::Prose;
                }
            }
            return Some(end.map_or(rest.len(), |end| end + 1));
        }
        if !is_complete && self.could_be_marker(body) {
            return None;
        }
        self.line = Line::Body;
        if self.block == Block::Code {
            return Some(0);
        }
        let hashes = body.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes) && body[hashes..].starts_with(' ') {
            self.line = Line::Heading;
            return Some(indent + hashes + 1);
        }
        for (marker, shown) in MARKERS {
            if body.starts_with(marker) {
                if indent > 0 {
                    out(Style::Plain, &head[..indent]);
                }
                out(Style::Marker, shown);
                return Some(indent + marker.len());
            }
        }
        Some(0)
    }

    fn could_be_marker(&self, body: &str) -> bool {
        if body.is_empty() || FENCE.starts_with(body) {
            return true;
        }
        if self.block == Block::Code {
            return false;
        }
        let is_hashes = body.len() <= 6 && body.bytes().all(|b| b == b'#');
        is_hashes || matches!(body, "-" | "*" | "+" | ">")
    }

    fn inline(
        &mut self,
        rest: &str,
        input: Input,
        out: &mut impl FnMut(Style, &str),
    ) -> Option<usize> {
        if self.block == Block::Code {
            let Some(end) = rest.find('\n') else {
                if input == Input::Streaming {
                    return None;
                }
                out(Style::Block, rest);
                return Some(rest.len());
            };
            out(Style::Block, &rest[..end]);
            out(Style::Plain, "\n");
            self.end_line();
            return Some(end + 1);
        }
        let mut run = 0;
        let mut chars = rest.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            match c {
                '\n' => {
                    self.emit(&rest[run..at], out);
                    out(Style::Plain, "\n");
                    self.end_line();
                    return Some(at + 1);
                }
                '`' => {
                    self.emit(&rest[run..at], out);
                    self.code = !self.code;
                    run = at + 1;
                }
                '*' if !self.code => match chars.peek() {
                    Some((_, '*')) => {
                        self.emit(&rest[run..at], out);
                        self.strong = !self.strong;
                        chars.next();
                        run = at + 2;
                    }
                    None if input == Input::Streaming => {
                        self.emit(&rest[run..at], out);
                        return (at > 0).then_some(at);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        self.emit(&rest[run..], out);
        Some(rest.len())
    }

    fn emit(&self, text: &str, out: &mut impl FnMut(Style, &str)) {
        if text.is_empty() {
            return;
        }
        let style = if self.code {
            Style::Code
        } else if self.line == Line::Heading {
            Style::Heading
        } else if self.strong {
            Style::Strong
        } else {
            Style::Plain
        };
        out(style, text);
    }

    fn end_line(&mut self) {
        self.line = Line::Start;
        self.strong = false;
        self.code = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(chunks: &[&str]) -> Vec<(Style, String)> {
        let mut pieces: Vec<(Style, String)> = Vec::new();
        let mut collect = |style: Style, text: &str| match pieces.last_mut() {
            Some((last, joined)) if *last == style => joined.push_str(text),
            _ => pieces.push((style, text.to_owned())),
        };
        let mut markdown = Markdown::default();
        for chunk in chunks {
            markdown.push(chunk, &mut collect);
        }
        markdown.finish(&mut collect);
        pieces
    }

    fn piece(style: Style, text: &str) -> (Style, String) {
        (style, text.to_owned())
    }

    #[test]
    fn strong_split_across_chunks_hides_its_markers() {
        assert_eq!(
            render(&["Use **bo", "ld*", "* now"]),
            [
                piece(Style::Plain, "Use "),
                piece(Style::Strong, "bold"),
                piece(Style::Plain, " now"),
            ]
        );
    }

    #[test]
    fn inline_code_is_styled_without_backticks() {
        assert_eq!(
            render(&["run `cargo test` first"]),
            [
                piece(Style::Plain, "run "),
                piece(Style::Code, "cargo test"),
                piece(Style::Plain, " first"),
            ]
        );
    }

    #[test]
    fn heading_and_list_markers_are_replaced() {
        assert_eq!(
            render(&["#", "# Plan\n", "-", " one\n"]),
            [
                piece(Style::Heading, "Plan"),
                piece(Style::Plain, "\n"),
                piece(Style::Marker, "• "),
                piece(Style::Plain, "one\n"),
            ]
        );
    }

    #[test]
    fn code_blocks_hide_their_fences_and_keep_markdown_characters() {
        assert_eq!(
            render(&["``", "`rust\nlet a = **b;\n- c\n```\nafter"]),
            [
                piece(Style::Open, "rust"),
                piece(Style::Block, "let a = **b;"),
                piece(Style::Plain, "\n"),
                piece(Style::Block, "- c"),
                piece(Style::Plain, "\n"),
                piece(Style::Close, ""),
                piece(Style::Plain, "after"),
            ]
        );
    }

    #[test]
    fn a_code_line_waits_for_its_end_while_streaming() {
        assert_eq!(
            render(&["```\nlet a", " = 1;\n```"]),
            [
                piece(Style::Open, ""),
                piece(Style::Block, "let a = 1;"),
                piece(Style::Plain, "\n"),
                piece(Style::Close, ""),
            ]
        );
    }

    #[test]
    fn styles_end_at_the_line_break() {
        assert_eq!(
            render(&["**open\nplain"]),
            [piece(Style::Strong, "open"), piece(Style::Plain, "\nplain"),]
        );
    }

    #[test]
    fn a_lone_trailing_star_is_kept_when_the_text_ends() {
        assert_eq!(render(&["2 *"]), [piece(Style::Plain, "2 *")]);
    }
}

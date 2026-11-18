use std::io::{self, Write};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::style::{Print, Stylize};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, execute, queue};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const PROMPT: &str = "› ";
const INDENT: &str = "  ";
const TAB: &str = "    ";

pub(crate) enum Entry {
    Line(String),
    Exit,
}

#[derive(Default)]
pub(crate) struct Editor {
    history: Vec<String>,
}

impl Editor {
    pub(crate) fn read(&mut self) -> io::Result<Entry> {
        let _raw = RawMode::enable()?;
        let mut out = io::stdout();
        let mut line = Line {
            recall: self.history.len(),
            ..Line::default()
        };
        line.render(&mut out)?;
        loop {
            let action = match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    line.key(key, &self.history)
                }
                Event::Paste(text) => {
                    line.insert(&clean_paste(&text));
                    Action::Edit
                }
                Event::Resize(..) => Action::Edit,
                _ => continue,
            };
            match action {
                Action::Edit => line.render(&mut out)?,
                Action::Submit => {
                    line.cursor = line.text.len();
                    line.render(&mut out)?;
                    execute!(out, Print("\r\n"))?;
                    if self.history.last() != Some(&line.text) {
                        self.history.push(line.text.clone());
                    }
                    return Ok(Entry::Line(line.text));
                }
                Action::Exit => {
                    execute!(out, Print("\r\n"))?;
                    return Ok(Entry::Exit);
                }
            }
        }
    }
}

enum Action {
    Edit,
    Submit,
    Exit,
}

#[derive(Default)]
struct Line {
    text: String,
    cursor: usize,
    cursor_row: usize,
    recall: usize,
    draft: String,
}

impl Line {
    fn key(&mut self, key: KeyEvent, history: &[String]) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Enter if alt => self.insert("\n"),
            KeyCode::Char('j') if ctrl => self.insert("\n"),
            KeyCode::Enter if self.text.trim().is_empty() => {}
            KeyCode::Enter => return Action::Submit,
            KeyCode::Char('c') if ctrl => {
                if self.text.is_empty() {
                    return Action::Exit;
                }
                self.text.clear();
                self.cursor = 0;
            }
            KeyCode::Char('d') if ctrl && self.text.is_empty() => return Action::Exit,
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Char('u') if ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Tab => self.insert(TAB),
            KeyCode::Char(c) if !ctrl => self.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace => {
                let start = self.previous_boundary();
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Delete => {
                let end = self.next_boundary();
                self.text.drain(self.cursor..end);
            }
            KeyCode::Left => self.cursor = self.previous_boundary(),
            KeyCode::Right => self.cursor = self.next_boundary(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Up => self.recall(history, -1),
            KeyCode::Down => self.recall(history, 1),
            _ => {}
        }
        Action::Edit
    }

    fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn previous_boundary(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(at, _)| at)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |g| self.cursor + g.len())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |at| at + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |at| self.cursor + at)
    }

    fn delete_word(&mut self) {
        let trimmed = self.text[..self.cursor].trim_end();
        let start = trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(at, c)| at + c.len_utf8());
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    fn recall(&mut self, history: &[String], step: isize) {
        let Some(next) = self.recall.checked_add_signed(step) else {
            return;
        };
        if next > history.len() {
            return;
        }
        if self.recall == history.len() {
            self.draft.clone_from(&self.text);
        }
        self.recall = next;
        self.text = history.get(next).unwrap_or(&self.draft).clone();
        self.cursor = self.text.len();
    }

    fn render(&mut self, out: &mut impl Write) -> io::Result<()> {
        let columns = terminal::size()
            .ok()
            .map(|(columns, _)| columns)
            .filter(|columns| *columns > 0)
            .unwrap_or(80);
        self.draw(out, usize::from(columns))
    }

    fn draw(&mut self, out: &mut impl Write, columns: usize) -> io::Result<()> {
        let width = columns.max(PROMPT.width() + 1);
        queue!(out, cursor::MoveToColumn(0))?;
        if self.cursor_row > 0 {
            queue!(
                out,
                cursor::MoveUp(u16::try_from(self.cursor_row).unwrap_or(u16::MAX))
            )?;
        }
        queue!(
            out,
            terminal::Clear(ClearType::FromCursorDown),
            Print(PROMPT.dim())
        )?;

        let (mut row, mut col) = (0, PROMPT.width());
        let mut at_cursor = None;
        for (index, grapheme) in self.text.grapheme_indices(true) {
            if grapheme == "\n" {
                if index == self.cursor {
                    at_cursor = Some((row, col));
                }
                queue!(out, Print("\r\n"), Print(INDENT))?;
                (row, col) = (row + 1, INDENT.width());
                continue;
            }
            let grapheme_width = grapheme.width();
            if col + grapheme_width > width {
                queue!(out, Print("\r\n"))?;
                (row, col) = (row + 1, 0);
            }
            if index == self.cursor {
                at_cursor = Some((row, col));
            }
            queue!(out, Print(grapheme))?;
            col += grapheme_width;
        }
        let (mut cursor_row, mut cursor_col) = at_cursor.unwrap_or((row, col));
        if cursor_col >= width {
            queue!(out, Print("\r\n"))?;
            row += 1;
            (cursor_row, cursor_col) = (row, 0);
        }
        if row > cursor_row {
            queue!(
                out,
                cursor::MoveUp(u16::try_from(row - cursor_row).unwrap_or(u16::MAX))
            )?;
        }
        queue!(
            out,
            cursor::MoveToColumn(u16::try_from(cursor_col).unwrap_or(u16::MAX))
        )?;
        self.cursor_row = cursor_row;
        out.flush()
    }
}

fn clean_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', TAB)
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}

struct RawMode;

impl RawMode {
    fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnableBracketedPaste)?;
        Ok(guard)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        execute!(io::stdout(), DisableBracketedPaste).ok();
        terminal::disable_raw_mode().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, cursor: usize) -> Line {
        Line {
            text: text.to_owned(),
            cursor,
            ..Line::default()
        }
    }

    fn press(line: &mut Line, code: KeyCode) {
        line.key(KeyEvent::new(code, KeyModifiers::NONE), &[]);
    }

    #[test]
    fn backspace_removes_a_whole_grapheme() {
        let mut line = line("a한", "a한".len());
        press(&mut line, KeyCode::Backspace);
        assert_eq!((line.text.as_str(), line.cursor), ("a", 1));
    }

    #[test]
    fn ctrl_w_deletes_the_previous_word() {
        let mut line = line("cargo test now", 14);
        line.key(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            &[],
        );
        assert_eq!(line.text, "cargo test ");
    }

    #[test]
    fn history_recall_keeps_the_draft() {
        let history = ["first".to_owned(), "second".to_owned()];
        let mut line = Line {
            recall: history.len(),
            ..line("draft", 5)
        };
        line.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &history);
        assert_eq!(line.text, "second");
        line.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &history);
        assert_eq!(line.text, "draft");
    }

    #[test]
    fn render_wraps_and_places_the_cursor_on_the_right_row() {
        let mut line = line("hello\nworld", 3);
        line.draw(&mut Vec::new(), 80).unwrap();
        assert_eq!(line.cursor_row, 0);
        let mut end = Line { cursor: 11, ..line };
        end.draw(&mut Vec::new(), 80).unwrap();
        assert_eq!(end.cursor_row, 1);
        let mut narrow = Line {
            text: "abcdefghij".to_owned(),
            cursor: 10,
            ..Line::default()
        };
        narrow.draw(&mut Vec::new(), 6).unwrap();
        assert_eq!(narrow.cursor_row, 2);
        let mut boundary = Line {
            cursor: 4,
            ..narrow
        };
        boundary.draw(&mut Vec::new(), 6).unwrap();
        assert_eq!(boundary.cursor_row, 1);
    }

    #[test]
    fn ctrl_w_after_wide_whitespace_keeps_char_boundaries() {
        let text = "가\u{3000}나다";
        let mut line = line(text, text.len());
        line.key(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            &[],
        );
        assert_eq!(line.text, "가\u{3000}");
    }

    #[test]
    fn pasted_tabs_and_control_characters_are_cleaned() {
        assert_eq!(clean_paste("a\tb\r\nc\u{7}"), "a    b\nc");
    }
}

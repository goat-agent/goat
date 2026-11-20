use std::collections::VecDeque;
use std::io::{self, Write};
use std::mem;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal;
use tokio::sync::mpsc;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::command::{self, Name, Typing};
use crate::render::{Frame, Spot, Terminal};
use crate::target::Choices;

const PROMPT: &str = "› ";
const INDENT: &str = "  ";
const TAB: &str = "    ";
const PALETTE_ROWS: usize = 12;
const KEY_POLL: Duration = Duration::from_millis(100);
const KEY_BACKLOG: usize = 64;

pub(crate) enum Entry {
    Line(String),
    Exit,
}

pub(crate) enum TurnKeys {
    Continue,
    Interrupt,
}

pub(crate) struct Keys {
    events: mpsc::Receiver<io::Result<Event>>,
    reader: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub(crate) struct Editor {
    history: Vec<String>,
    queue: VecDeque<String>,
    line: Line,
}

enum Action {
    Edit,
    Submit,
    Exit,
}

#[derive(Clone, Copy)]
struct Pick {
    selected: Option<usize>,
    effort: Option<usize>,
}

#[derive(Default)]
struct Line {
    text: String,
    cursor: usize,
    recall: usize,
    draft: String,
    selected: Option<usize>,
    effort: Option<usize>,
}

struct Item {
    label: String,
    is_current: bool,
    text: String,
    is_final: bool,
    efforts: Vec<String>,
    effort: usize,
}

impl Keys {
    pub(crate) fn start() -> Self {
        let (sender, events) = mpsc::channel(KEY_BACKLOG);
        let reader = thread::spawn(move || read_events(&sender));
        Self {
            events,
            reader: Some(reader),
        }
    }

    pub(crate) async fn next(&mut self) -> io::Result<Event> {
        self.events
            .recv()
            .await
            .unwrap_or_else(|| Err(io::Error::new(io::ErrorKind::UnexpectedEof, "input closed")))
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.events.close();
        if let Some(reader) = self.reader.take() {
            reader.join().ok();
        }
    }
}

impl Editor {
    pub(crate) async fn read(
        &mut self,
        keys: &mut Keys,
        choices: &Choices<'_>,
        terminal: &mut Terminal<impl Write>,
    ) -> io::Result<Entry> {
        if let Some(text) = self.queue.pop_front() {
            return Ok(self.send(text, choices, terminal));
        }
        terminal.show(self.frame(terminal.columns(), choices));
        loop {
            let action = match keys.next().await? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    self.line.key(key, &self.history, choices)
                }
                Event::Paste(text) => {
                    self.line.insert(&clean_paste(&text));
                    Action::Edit
                }
                Event::Resize(..) => Action::Edit,
                _ => continue,
            };
            match action {
                Action::Edit => terminal.show(self.frame(terminal.columns(), choices)),
                Action::Submit => {
                    let text = self.take();
                    return Ok(self.send(text, choices, terminal));
                }
                Action::Exit => {
                    terminal.close();
                    return Ok(Entry::Exit);
                }
            }
        }
    }

    pub(crate) fn during_turn(
        &mut self,
        event: Event,
        choices: &Choices<'_>,
        terminal: &mut Terminal<impl Write>,
    ) -> TurnKeys {
        match event {
            Event::Paste(text) => self.line.insert(&clean_paste(&text)),
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if is_interrupt(key) {
                    return TurnKeys::Interrupt;
                }
                if let Action::Submit = self.line.key(key, &self.history, choices) {
                    let text = self.take();
                    self.queue.push_back(text);
                }
            }
            Event::Resize(..) => {}
            _ => return TurnKeys::Continue,
        }
        terminal.show(self.frame(terminal.columns(), choices));
        TurnKeys::Continue
    }

    pub(crate) fn unqueue(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let mut texts: Vec<String> = self.queue.drain(..).collect();
        if !self.line.text.is_empty() {
            texts.push(mem::take(&mut self.line.text));
        }
        self.line.text = texts.join("\n");
        self.line.cursor = self.line.text.len();
    }

    fn take(&mut self) -> String {
        let text = mem::take(&mut self.line).text;
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        self.line.recall = self.history.len();
        text
    }

    fn send(
        &mut self,
        text: String,
        choices: &Choices<'_>,
        terminal: &mut Terminal<impl Write>,
    ) -> Entry {
        let columns = terminal.columns();
        terminal.echo(&echo(&text, columns));
        terminal.show(self.frame(columns, choices));
        Entry::Line(text)
    }

    fn frame(&self, columns: usize, choices: &Choices<'_>) -> Frame {
        let items = palette(&self.line.text, choices);
        let rows = self.line.rows(&items);
        let selected = self
            .line
            .selected
            .or(preselected(&self.line.text, rows.len()));
        let mut frame = self.line.frame(columns, &rows, selected);
        let room = width(columns) - 1 - PROMPT.width();
        let queued = self.queue.iter().map(|text| {
            let first = text.lines().next().unwrap_or_default();
            format!("{}{}", PROMPT.dim(), fit(first, room).dim())
        });
        frame.rows.splice(0..0, queued);
        frame.cursor.row += self.queue.len();
        frame
    }
}

impl Line {
    fn key(&mut self, key: KeyEvent, history: &[String], choices: &Choices<'_>) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        let selected = self.selected.take();
        let effort = self.effort.take();
        let pick = Pick { selected, effort };
        if let Some(action) = self.choose(key.code, alt, pick, history, choices) {
            return action;
        }
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
            KeyCode::Tab => {
                if !self.complete() {
                    self.insert(TAB);
                }
            }
            KeyCode::Char(c) if !ctrl => self.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace => self.backspace(),
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

    fn backspace(&mut self) {
        let start = self.previous_boundary();
        self.text.drain(start..self.cursor);
        self.cursor = start;
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

    fn choose(
        &mut self,
        code: KeyCode,
        alt: bool,
        pick: Pick,
        history: &[String],
        choices: &Choices<'_>,
    ) -> Option<Action> {
        let is_moving = matches!(code, KeyCode::Up | KeyCode::Down);
        let is_turning = matches!(code, KeyCode::Left | KeyCode::Right) && pick.selected.is_some();
        let is_taking = match code {
            KeyCode::Enter => !alt,
            KeyCode::Tab => true,
            _ => false,
        };
        if !(is_moving && self.recall == history.len() || is_turning || is_taking) {
            return None;
        }
        let mut items = palette(&self.text, choices);
        let count = items.len();
        if count == 0 {
            return None;
        }
        if is_turning {
            let at = pick.selected?.min(count - 1);
            let item = &items[at];
            if item.efforts.is_empty() {
                return None;
            }
            let effort = pick.effort.unwrap_or(item.effort);
            self.selected = Some(at);
            self.effort = Some(match code {
                KeyCode::Left => effort.saturating_sub(1),
                _ => (effort + 1).min(item.efforts.len() - 1),
            });
            return Some(Action::Edit);
        }
        let selected = pick.selected.or(preselected(&self.text, count));
        if is_moving {
            self.selected = Some(match (code, selected) {
                (KeyCode::Down, None) => 0,
                (KeyCode::Down, Some(at)) => (at + 1) % count,
                (_, None) => count - 1,
                (_, Some(at)) => (at + count - 1) % count,
            });
            return Some(Action::Edit);
        }
        let item = items.swap_remove(selected?.min(count - 1));
        self.text = item.text;
        if let Some(effort) = pick.effort
            && effort != item.effort
            && let Some(level) = item.efforts.get(effort)
        {
            self.text.push(':');
            self.text.push_str(level);
        }
        self.cursor = self.text.len();
        if code == KeyCode::Enter && item.is_final {
            return Some(Action::Submit);
        }
        Some(Action::Edit)
    }

    fn complete(&mut self) -> bool {
        let typed = match command::typing(&self.text) {
            Typing::Name(typed) => typed,
            Typing::Argument(..) => return true,
            Typing::Message => return false,
        };
        if self.cursor != self.text.len() {
            return true;
        }
        let keep = self.text.len() - typed.len();
        if let Some((extended, is_unique)) = shared_start(command::words(), typed) {
            self.text.truncate(keep);
            self.text.push_str(&extended);
            if is_unique {
                self.text.push(' ');
            }
            self.cursor = self.text.len();
        }
        true
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

    fn rows(&self, items: &[Item]) -> Vec<String> {
        let shown: Vec<String> = items
            .iter()
            .map(|item| {
                if item.is_current {
                    format!("{} (current)", item.label)
                } else {
                    item.label.clone()
                }
            })
            .collect();
        let width = shown.iter().map(|row| row.width()).max().unwrap_or(0);
        shown
            .into_iter()
            .zip(items)
            .enumerate()
            .map(|(index, (row, item))| {
                let level = self
                    .selected
                    .filter(|selected| *selected == index)
                    .and_then(|_| item.efforts.get(self.effort.unwrap_or(item.effort)));
                match level {
                    Some(level) => {
                        let pad = " ".repeat(width - row.width());
                        format!("{INDENT}{row}{pad}  ‹ {level} ›")
                    }
                    None => format!("{INDENT}{row}"),
                }
            })
            .collect()
    }

    fn frame(&self, columns: usize, palette: &[String], selected: Option<usize>) -> Frame {
        let width = width(columns);
        let command = command::parse(&self.text).map(|run| run.span);
        let mut rows = Vec::new();
        let mut row = format!("{}", PROMPT.dim());
        let mut col = PROMPT.width();
        let mut at_cursor = None;
        for (index, grapheme) in self.text.grapheme_indices(true) {
            if grapheme == "\n" {
                if index == self.cursor {
                    at_cursor = Some((rows.len(), col));
                }
                rows.push(mem::replace(&mut row, INDENT.to_owned()));
                col = INDENT.width();
                continue;
            }
            let grapheme_width = grapheme.width();
            if col + grapheme_width > width {
                rows.push(mem::replace(&mut row, INDENT.to_owned()));
                col = INDENT.width();
            }
            if index == self.cursor {
                at_cursor = Some((rows.len(), col));
            }
            if command.as_ref().is_some_and(|span| span.contains(&index)) {
                row.push_str(&grapheme.cyan().to_string());
            } else {
                row.push_str(grapheme);
            }
            col += grapheme_width;
        }
        let (mut cursor_row, mut cursor_col) = at_cursor.unwrap_or((rows.len(), col));
        if cursor_col >= width {
            rows.push(mem::replace(&mut row, INDENT.to_owned()));
            (cursor_row, cursor_col) = (rows.len(), INDENT.width());
        }
        rows.push(row);
        for (index, row_text) in palette.iter().enumerate() {
            if selected == Some(index) {
                let text = row_text.strip_prefix(INDENT).unwrap_or(row_text);
                let shown = fit(text, width - 1 - PROMPT.width());
                rows.push(format!("{}{shown}", PROMPT.dim()));
            } else {
                rows.push(format!("{}", fit(row_text, width - 1).dim()));
            }
        }
        Frame {
            rows,
            cursor: Spot {
                row: cursor_row,
                column: cursor_col,
            },
        }
    }
}

fn palette(text: &str, choices: &Choices<'_>) -> Vec<Item> {
    let (name, typed) = match command::typing(text) {
        Typing::Name(typed) => {
            let head = &text[..text.len() - typed.len()];
            return command::SPECS
                .iter()
                .filter(|spec| spec.word.starts_with(typed))
                .map(|spec| {
                    let takes_value = matches!(spec.name, Name::Model | Name::Effort);
                    Item {
                        label: format!("/{:<10}{}", spec.word, spec.about),
                        is_current: false,
                        text: if takes_value {
                            format!("{head}{} ", spec.word)
                        } else {
                            format!("{head}{}", spec.word)
                        },
                        is_final: !takes_value,
                        efforts: Vec::new(),
                        effort: 0,
                    }
                })
                .collect();
        }
        Typing::Argument(name, typed) => (name, typed),
        Typing::Message => return Vec::new(),
    };
    let head = &text[..text.len() - typed.len()];
    let candidates = match name {
        Name::Model => choices.models(typed),
        Name::Effort => choices.efforts(typed),
        Name::Help | Name::Exit => Vec::new(),
    };
    candidates
        .into_iter()
        .take(PALETTE_ROWS)
        .map(|candidate| Item {
            is_final: !candidate.value.ends_with('/'),
            text: format!("{head}{}", candidate.value),
            label: candidate.label,
            is_current: candidate.is_current,
            efforts: candidate.efforts,
            effort: candidate.effort,
        })
        .collect()
}

fn preselected(text: &str, count: usize) -> Option<usize> {
    let has_query = match command::typing(text) {
        Typing::Name(_) => true,
        Typing::Argument(_, typed) => !typed.is_empty(),
        Typing::Message => false,
    };
    (count > 0 && has_query).then_some(0)
}

fn shared_start<'a>(options: impl Iterator<Item = &'a str>, typed: &str) -> Option<(String, bool)> {
    let mut matching = options.filter(|option| option.starts_with(typed));
    let mut shared = matching.next()?;
    let mut is_unique = true;
    for option in matching {
        is_unique = false;
        let end = shared
            .char_indices()
            .zip(option.chars())
            .find(|((_, a), b)| a != b)
            .map_or(shared.len().min(option.len()), |((at, _), _)| at);
        shared = &shared[..end];
    }
    Some((shared.to_owned(), is_unique))
}

fn echo(text: &str, columns: usize) -> Vec<String> {
    let line = Line {
        text: text.to_owned(),
        cursor: text.len(),
        ..Line::default()
    };
    line.frame(columns, &[], None).rows
}

fn width(columns: usize) -> usize {
    columns.max(PROMPT.width() + 2)
}

fn is_interrupt(key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Esc) || ctrl && matches!(key.code, KeyCode::Char('c'))
}

pub(crate) fn fit(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (at, grapheme) in text.grapheme_indices(true) {
        used += grapheme.width();
        if used > width {
            return &text[..at];
        }
    }
    text
}

fn read_events(sender: &mpsc::Sender<io::Result<Event>>) {
    while !sender.is_closed() {
        match event::poll(KEY_POLL) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(err) => {
                sender.blocking_send(Err(err)).ok();
                return;
            }
        }
        let event = event::read();
        let is_broken = event.is_err();
        if sender.blocking_send(event).is_err() || is_broken {
            return;
        }
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

pub(crate) struct RawMode;

impl RawMode {
    pub(crate) fn enable() -> io::Result<Self> {
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
    use goat_core::account::{Account, Credential, Kind};
    use goat_core::catalog::Model;

    use super::*;
    use crate::target::{Current, Offer};

    fn line(text: &str, cursor: usize) -> Line {
        Line {
            text: text.to_owned(),
            cursor,
            ..Line::default()
        }
    }

    fn with_choices<T>(use_choices: impl FnOnce(&Choices<'_>) -> T) -> T {
        let account = Account::new(
            Kind::OpenAi,
            "codex",
            Credential::ApiKey { key: "k".into() },
        );
        let catalog: Vec<Model> = ["gpt-6.2-sol", "gpt-6.2-spark", "gpt-6.2-astra"]
            .into_iter()
            .map(|id| Model {
                id: id.into(),
                kind: Kind::OpenAi,
                display_name: None,
                efforts: (id == "gpt-6.2-sol").then(|| vec!["low".into(), "high".into()]),
                default_effort: None,
            })
            .collect();
        let offers = [Offer {
            account: account.id,
            kind: Kind::OpenAi,
            label: account.label(),
        }];
        use_choices(&Choices {
            offers: &offers,
            catalog: &catalog,
            current: Current {
                account: account.id,
                model: "gpt-6.2-sol",
                effort: None,
            },
        })
    }

    fn key(line: &mut Line, code: KeyCode, modifiers: KeyModifiers, history: &[String]) {
        with_choices(|choices| line.key(KeyEvent::new(code, modifiers), history, choices));
    }

    fn rows(text: &str) -> Vec<String> {
        with_choices(|choices| Line::default().rows(&palette(text, choices)))
    }

    fn press(line: &mut Line, codes: &[KeyCode]) -> Action {
        let mut action = Action::Edit;
        for code in codes {
            action = with_choices(|choices| {
                line.key(KeyEvent::new(*code, KeyModifiers::NONE), &[], choices)
            });
        }
        action
    }

    fn tab(text: &str) -> String {
        let mut line = line(text, text.len());
        key(&mut line, KeyCode::Tab, KeyModifiers::NONE, &[]);
        line.text
    }

    fn screen(text: &str) -> String {
        line(text, text.len()).frame(80, &[], None).rows.join("\n")
    }

    fn cursor(text: &str, at: usize, columns: usize) -> Spot {
        line(text, at).frame(columns, &[], None).cursor
    }

    fn during_turn(editor: &mut Editor, code: KeyCode) -> TurnKeys {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        let event = Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        with_choices(|choices| editor.during_turn(event, choices, &mut terminal))
    }

    #[test]
    fn backspace_removes_a_whole_grapheme() {
        let mut line = line("a한", "a한".len());
        key(&mut line, KeyCode::Backspace, KeyModifiers::NONE, &[]);
        assert_eq!((line.text.as_str(), line.cursor), ("a", 1));
    }

    #[test]
    fn ctrl_w_deletes_the_previous_word() {
        let mut line = line("cargo test now", 14);
        key(&mut line, KeyCode::Char('w'), KeyModifiers::CONTROL, &[]);
        assert_eq!(line.text, "cargo test ");
    }

    #[test]
    fn ctrl_w_after_wide_whitespace_keeps_char_boundaries() {
        let text = "가\u{3000}나다";
        let mut line = line(text, text.len());
        key(&mut line, KeyCode::Char('w'), KeyModifiers::CONTROL, &[]);
        assert_eq!(line.text, "가\u{3000}");
    }

    #[test]
    fn history_recall_keeps_the_draft() {
        let history = ["first".to_owned(), "second".to_owned()];
        let mut line = Line {
            recall: history.len(),
            ..line("draft", 5)
        };
        key(&mut line, KeyCode::Up, KeyModifiers::NONE, &history);
        assert_eq!(line.text, "second");
        key(&mut line, KeyCode::Down, KeyModifiers::NONE, &history);
        assert_eq!(line.text, "draft");
    }

    #[test]
    fn the_cursor_sits_on_the_row_of_its_line() {
        assert_eq!(cursor("hello\nworld", 3, 80), Spot { row: 0, column: 5 });
        assert_eq!(cursor("hello\nworld", 11, 80), Spot { row: 1, column: 7 });
    }

    #[test]
    fn the_cursor_follows_wrapped_rows() {
        assert_eq!(cursor("abcdefghij", 10, 6), Spot { row: 2, column: 4 });
        assert_eq!(cursor("abcdefghij", 4, 6), Spot { row: 1, column: 2 });
    }

    #[test]
    fn wrapped_rows_line_up_under_the_text() {
        let rows = line("abcdefgh", 8).frame(6, &[], None).rows;
        assert_eq!(rows[1..], ["  efgh", "  "]);
    }

    #[test]
    fn enter_during_a_turn_queues_the_line_above_the_prompt() {
        let mut editor = Editor {
            line: line("next", 4),
            ..Editor::default()
        };

        during_turn(&mut editor, KeyCode::Enter);

        let rows = with_choices(|choices| editor.frame(40, choices).rows);
        let queued = format!("{}{}", PROMPT.dim(), "next".dim());
        assert_eq!(rows, [queued, format!("{}", PROMPT.dim())]);
    }

    #[test]
    fn esc_during_a_turn_interrupts_it() {
        let mut editor = Editor::default();

        let keys = during_turn(&mut editor, KeyCode::Esc);

        assert!(matches!(keys, TurnKeys::Interrupt));
    }

    #[test]
    fn unqueueing_puts_queued_lines_back_before_the_draft() {
        let mut editor = Editor {
            queue: ["first".to_owned(), "second".to_owned()].into(),
            line: line("draft", 5),
            ..Editor::default()
        };

        editor.unqueue();

        assert_eq!(editor.line.text, "first\nsecond\ndraft");
        assert!(editor.queue.is_empty());
    }

    #[test]
    fn pasted_tabs_and_control_characters_are_cleaned() {
        assert_eq!(clean_paste("a\tb\r\nc\u{7}"), "a    b\nc");
    }

    #[test]
    fn tab_takes_the_picked_command() {
        assert_eq!(tab("/ex"), "/exit");
    }

    #[test]
    fn tab_completes_an_alias() {
        assert_eq!(tab("/q"), "/quit ");
    }

    #[test]
    fn tab_takes_the_first_match_of_a_part() {
        assert_eq!(tab("/model spa"), "/model gpt-6.2-spark");
    }

    #[test]
    fn tab_takes_a_value_without_a_space() {
        assert_eq!(tab("/model gpt-6.2-a"), "/model gpt-6.2-astra");
    }

    #[test]
    fn tab_outside_a_command_inserts_spaces() {
        assert_eq!(tab("hi"), "hi    ");
    }

    #[test]
    fn slash_lists_matching_commands_under_the_prompt() {
        assert_eq!(
            rows("/e"),
            ["  /effort    Set the reasoning effort", "  /exit      Quit"]
        );
    }

    #[test]
    fn a_command_lists_its_values_and_marks_the_current_one() {
        assert_eq!(
            rows("/model gpt-6.2-s"),
            ["  gpt-6.2-sol (current)", "  gpt-6.2-spark"]
        );
    }

    #[test]
    fn a_message_shows_no_palette() {
        assert_eq!(rows("hello /model"), Vec::<String>::new());
    }

    #[test]
    fn a_known_command_is_highlighted() {
        assert!(screen("/model gpt").contains(&format!("{}", "/".cyan())));
        assert!(!screen("/modle gpt").contains(&format!("{}", "/".cyan())));
    }

    #[test]
    fn long_rows_are_cut_to_the_width() {
        assert_eq!(fit("가나다라", 5), "가나");
    }

    #[test]
    fn arrows_pick_a_command_and_enter_runs_it() {
        let mut line = line("/", 1);
        let action = press(&mut line, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        assert!(matches!(action, Action::Submit));
        assert_eq!(line.text, "/help");
    }

    #[test]
    fn enter_on_a_lone_slash_opens_the_first_command() {
        let mut line = line("/", 1);
        let action = press(&mut line, &[KeyCode::Enter]);
        assert!(matches!(action, Action::Edit));
        assert_eq!(line.text, "/model ");
    }

    #[test]
    fn enter_after_a_value_list_runs_what_was_typed() {
        let mut line = line("/model gpt-6.2-sol", 18);
        let action = press(&mut line, &[KeyCode::Enter]);
        assert!(matches!(action, Action::Submit));
        assert_eq!(line.text, "/model gpt-6.2-sol");
    }

    #[test]
    fn enter_on_a_part_of_a_name_takes_the_first_match() {
        let mut line = line("/model spa", 10);
        let action = press(&mut line, &[KeyCode::Enter]);
        assert!(matches!(action, Action::Submit));
        assert_eq!(line.text, "/model gpt-6.2-spark");
    }

    #[test]
    fn the_picked_row_is_marked_with_the_prompt_sign() {
        let rows = ["  /model".to_owned(), "  /help".to_owned()];
        let shown = line("/", 1).frame(80, &rows, Some(1)).rows;
        assert_eq!(shown[1], format!("{}", "  /model".dim()));
        assert_eq!(shown[2], format!("{}/help", PROMPT.dim()));
    }

    #[test]
    fn picking_a_model_runs_the_switch() {
        let mut line = line("/model gpt", 10);
        let action = press(&mut line, &[KeyCode::Up, KeyCode::Enter]);
        assert!(matches!(action, Action::Submit));
        assert_eq!(line.text, "/model gpt-6.2-astra");
    }

    #[test]
    fn picking_an_account_waits_for_its_model() {
        let mut line = line("/model o", 8);
        let action = press(&mut line, &[KeyCode::Down, KeyCode::Enter]);
        assert!(matches!(action, Action::Edit));
        assert_eq!(line.text, "/model openai:codex/");
    }

    #[test]
    fn typing_drops_the_pick() {
        let mut line = line("/", 1);
        press(&mut line, &[KeyCode::Down, KeyCode::Char('e')]);
        assert_eq!(line.selected, None);
    }

    #[test]
    fn arrows_outside_a_list_recall_history() {
        let history = ["first".to_owned()];
        let mut line = Line {
            recall: history.len(),
            ..line("draft", 5)
        };
        key(&mut line, KeyCode::Up, KeyModifiers::NONE, &history);
        assert_eq!(line.text, "first");
    }

    #[test]
    fn right_raises_the_effort_of_the_picked_model() {
        let mut line = line("/model sol", 10);
        let keys = [
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::Right,
            KeyCode::Right,
            KeyCode::Enter,
        ];
        let action = press(&mut line, &keys);
        assert!(matches!(action, Action::Submit));
        assert_eq!(line.text, "/model gpt-6.2-sol:high");
    }

    #[test]
    fn the_picked_model_shows_its_effort() {
        let mut line = line("/model sol", 10);
        press(&mut line, &[KeyCode::Down, KeyCode::Right]);
        let rows = with_choices(|choices| line.rows(&palette(&line.text, choices)));
        assert_eq!(rows, ["  gpt-6.2-sol (current)  ‹ low ›"]);
    }

    #[test]
    fn left_and_right_move_the_cursor_until_a_row_is_picked() {
        let mut line = line("/model sol", 10);
        press(&mut line, &[KeyCode::Left]);
        assert_eq!(line.cursor, 9);
    }

    #[test]
    fn an_unchanged_effort_is_left_out() {
        let mut line = line("/model sol", 10);
        let keys = [KeyCode::Down, KeyCode::Right, KeyCode::Left, KeyCode::Enter];
        let action = press(&mut line, &keys);
        assert!(matches!(action, Action::Submit));
        assert_eq!(line.text, "/model gpt-6.2-sol");
    }
}

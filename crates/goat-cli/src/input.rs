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

use goat_core::image::{Image, Picture};
use goat_core::store::Store;
use goat_core::tool::{Decision, Scope};

use crate::attach::{self, Tile};
use crate::command::{self, Name, Typing};
use crate::kitty;
use crate::render::{Frame, Placement, Spot, Terminal};
use crate::target::Choices;

const PROMPT: &str = "› ";
const SHELL_PROMPT: &str = "! ";
const INDENT: &str = "  ";
const TAB: &str = "    ";
const PALETTE_ROWS: usize = 12;
const LABEL_COLUMN: usize = 30;
const REWIND_HINT: &str = "Esc again to go back to an earlier prompt";
const KEY_POLL: Duration = Duration::from_millis(100);
const KEY_BACKLOG: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Agent,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Input {
    pub(crate) mode: Mode,
    pub(crate) text: String,
    pub(crate) images: Vec<Picture>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Past {
    mode: Mode,
    text: String,
    images: Vec<(usize, Image)>,
}

pub(crate) enum Entry {
    Input(Input),
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
    history: Vec<Past>,
    queue: VecDeque<Input>,
    line: Line,
    images: usize,
    store: Option<Store>,
}

enum Action {
    Edit,
    Submit,
    Clipboard,
    Exit,
}

#[derive(Clone, Copy)]
struct Pick {
    selected: Option<usize>,
    effort: Option<usize>,
}

#[derive(Default)]
struct Line {
    mode: Mode,
    text: String,
    cursor: usize,
    recall: usize,
    draft: Past,
    selected: Option<usize>,
    effort: Option<usize>,
    attachments: Vec<Attachment>,
    recalled: Vec<(usize, Image)>,
    armed: bool,
}

struct Attachment {
    number: usize,
    picture: Picture,
    tile: Option<Tile>,
}

struct Item {
    label: String,
    about: String,
    is_current: bool,
    text: String,
    cursor: Option<usize>,
    is_final: bool,
    efforts: Vec<String>,
    effort: usize,
}

impl Input {
    pub(crate) fn plain(&self) -> String {
        let mut plain = String::with_capacity(self.text.len());
        for c in self.text.chars() {
            match image_of_char(c) {
                Some(number) => plain.push_str(&label(number)),
                None => plain.push(c),
            }
        }
        plain
    }
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
    pub(crate) fn new(store: Store) -> Self {
        Self {
            store: Some(store),
            ..Self::default()
        }
    }

    pub(crate) async fn read(
        &mut self,
        keys: &mut Keys,
        choices: &Choices<'_>,
        terminal: &mut Terminal<impl Write>,
    ) -> io::Result<Entry> {
        if let Some(input) = self.queue.pop_front() {
            return Ok(self.send(input, choices, terminal));
        }
        self.show(choices, terminal);
        loop {
            let action = match keys.next().await? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    self.line.key(key, &self.history, choices)
                }
                Event::Paste(text) => {
                    self.paste(&text, terminal);
                    Action::Edit
                }
                Event::Resize(..) => Action::Edit,
                _ => continue,
            };
            match action {
                Action::Edit => self.show(choices, terminal),
                Action::Clipboard => {
                    self.paste_clipboard(terminal);
                    self.show(choices, terminal);
                }
                Action::Submit => {
                    let input = self.take();
                    return Ok(self.send(input, choices, terminal));
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
            Event::Paste(text) => self.paste(&text, terminal),
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if is_interrupt(key) {
                    return TurnKeys::Interrupt;
                }
                match self.line.key(key, &self.history, choices) {
                    Action::Submit => {
                        let input = self.take();
                        self.queue.push_back(input);
                    }
                    Action::Clipboard => self.paste_clipboard(terminal),
                    Action::Edit | Action::Exit => {}
                }
            }
            Event::Resize(..) => {}
            _ => return TurnKeys::Continue,
        }
        self.show(choices, terminal);
        TurnKeys::Continue
    }

    pub(crate) fn prefill(&mut self, text: &str, images: &[Image]) {
        let mut shown = String::with_capacity(text.len());
        let mut numbers: Vec<(usize, usize)> = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find(IMAGE_LABEL) {
            let tail = &rest[at + IMAGE_LABEL.len()..];
            let old = tail
                .find(']')
                .and_then(|end| Some((tail[..end].parse::<usize>().ok()?, end)));
            let Some((old, end)) = old else {
                shown.push_str(&rest[..at + IMAGE_LABEL.len()]);
                rest = tail;
                continue;
            };
            let number = if let Some((_, number)) = numbers.iter().find(|(seen, _)| *seen == old) {
                *number
            } else {
                self.images += 1;
                numbers.push((old, self.images));
                self.images
            };
            shown.push_str(&rest[..at]);
            shown.push(marker(number));
            rest = &tail[end + 1..];
        }
        shown.push_str(rest);
        self.line = Line {
            cursor: shown.len(),
            text: shown,
            recall: self.history.len(),
            recalled: numbers
                .iter()
                .zip(images)
                .map(|((_, number), image)| (*number, image.clone()))
                .collect(),
            ..Line::default()
        };
    }

    pub(crate) fn unqueue(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let mut inputs: Vec<Input> = self.queue.drain(..).collect();
        if !self.line.text.is_empty() {
            inputs.push(Input {
                mode: self.line.mode,
                text: mem::take(&mut self.line.text),
                images: Vec::new(),
            });
        }
        for input in &mut inputs {
            let numbers = images_in(&input.text);
            for (number, picture) in numbers.into_iter().zip(input.images.drain(..)) {
                self.line.attachments.push(Attachment {
                    number,
                    picture,
                    tile: None,
                });
            }
        }
        let is_shell = inputs.iter().all(|input| input.mode == Mode::Shell);
        let lines: Vec<String> = inputs
            .into_iter()
            .map(|input| match (is_shell, input.mode) {
                (false, Mode::Shell) => format!("!{}", input.text),
                _ => input.text,
            })
            .collect();
        self.line.mode = if is_shell { Mode::Shell } else { Mode::Agent };
        self.line.text = lines.join("\n");
        self.line.cursor = self.line.text.len();
    }

    fn take(&mut self) -> Input {
        let mut line = mem::take(&mut self.line);
        let mut images = Vec::new();
        let mut kept = Vec::new();
        for number in images_in(&line.text) {
            let Some(at) = line
                .attachments
                .iter()
                .position(|attachment| attachment.number == number)
            else {
                continue;
            };
            let picture = line.attachments.swap_remove(at).picture;
            if let Some(store) = &self.store {
                store.put_image(&picture).ok();
            }
            kept.push((number, picture.image.clone()));
            images.push(picture);
        }
        let past = Past {
            mode: line.mode,
            text: line.text.clone(),
            images: kept,
        };
        if self.history.last() != Some(&past) {
            self.history.push(past);
        }
        self.line.recall = self.history.len();
        Input {
            mode: line.mode,
            text: line.text,
            images,
        }
    }

    fn paste(&mut self, text: &str, terminal: &mut Terminal<impl Write>) {
        let paths = match self.line.mode {
            Mode::Agent => attach::pasted_images(text),
            Mode::Shell => None,
        };
        let Some(paths) = paths else {
            self.line.insert(&clean_paste(text));
            return;
        };
        for path in paths {
            match attach::load(&path) {
                Ok(picture) => self.attach(picture),
                Err(problem) => terminal.note(&problem),
            }
        }
    }

    fn paste_clipboard(&mut self, terminal: &mut Terminal<impl Write>) {
        match attach::clipboard() {
            Ok(Some(picture)) => self.attach(picture),
            Ok(None) => terminal.note("There is no image in the clipboard"),
            Err(problem) => terminal.note(&problem),
        }
    }

    fn attach(&mut self, picture: Picture) {
        self.images += 1;
        self.line.insert(&format!("{} ", marker(self.images)));
        self.line.attachments.push(Attachment {
            number: self.images,
            picture,
            tile: None,
        });
    }

    fn restore(&mut self) {
        for (number, image) in mem::take(&mut self.line.recalled) {
            let is_attached = self
                .line
                .attachments
                .iter()
                .any(|attachment| attachment.number == number);
            let picture = self
                .store
                .as_ref()
                .filter(|_| !is_attached)
                .and_then(|store| store.image(&image.id).ok().flatten())
                .and_then(|bytes| goat_core::image::prepare(bytes).ok());
            if let Some(picture) = picture {
                self.line.attachments.push(Attachment {
                    number,
                    picture,
                    tile: None,
                });
            }
        }
    }

    fn show(&mut self, choices: &Choices<'_>, terminal: &mut Terminal<impl Write>) {
        self.restore();
        if let Some(cell) = terminal.graphics() {
            let text = &self.line.text;
            for attachment in &mut self.line.attachments {
                if attachment.tile.is_some() || !text.contains(marker(attachment.number)) {
                    continue;
                }
                if let Some((png, columns)) = attach::thumbnail(&attachment.picture, cell) {
                    let id = tile_id(attachment.number);
                    terminal.write_raw(&kitty::transmit(id, &png));
                    attachment.tile = Some(Tile { id, columns });
                }
            }
        }
        terminal.show(self.frame(terminal.columns(), choices));
    }

    fn send(
        &mut self,
        input: Input,
        choices: &Choices<'_>,
        terminal: &mut Terminal<impl Write>,
    ) -> Entry {
        terminal.echo(&echo(&input, terminal.columns()));
        self.show(choices, terminal);
        Entry::Input(input)
    }

    fn frame(&self, columns: usize, choices: &Choices<'_>) -> Frame {
        let items = match self.line.mode {
            Mode::Agent => palette(&self.line.text, self.line.cursor, choices),
            Mode::Shell => Vec::new(),
        };
        let rows = self.line.rows(&items);
        let selected =
            self.line
                .selected
                .or(preselected(&self.line.text, self.line.cursor, rows.len()));
        let mut frame = self.line.frame(columns, &rows, selected);
        let room = width(columns) - 1 - PROMPT.width();
        let queued: Vec<String> = self
            .queue
            .iter()
            .map(|input| {
                let plain = input.plain();
                let first = plain.lines().next().unwrap_or_default();
                format!("{}{}", sign(input.mode).dim(), fit(first, room).dim())
            })
            .collect();
        let tiles = self.tiles(columns, queued.len());
        let blank = if tiles.is_empty() {
            0
        } else {
            usize::from(kitty::ROWS)
        };
        let above = queued.len() + blank;
        frame.rows.splice(
            0..0,
            queued.into_iter().chain((0..blank).map(|_| String::new())),
        );
        frame.cursor.row += above;
        frame.images = tiles;
        frame
    }

    fn tiles(&self, columns: usize, row: usize) -> Vec<Placement> {
        let mut column = INDENT.width();
        let mut placed = Vec::new();
        for attachment in &self.line.attachments {
            let Some(tile) = attachment.tile else {
                continue;
            };
            let end = column + usize::from(tile.columns);
            if !self.line.text.contains(marker(attachment.number)) || end > width(columns) {
                continue;
            }
            placed.push(Placement {
                row,
                column: u16::try_from(column).unwrap_or(u16::MAX),
                id: tile.id,
                columns: tile.columns,
            });
            column = end + 1;
        }
        placed
    }
}

impl Line {
    fn key(&mut self, key: KeyEvent, history: &[Past], choices: &Choices<'_>) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        let selected = self.selected.take();
        let effort = self.effort.take();
        let pick = Pick { selected, effort };
        let armed = mem::take(&mut self.armed);
        if let Some(action) = self.choose(key.code, alt, pick, history, choices) {
            return action;
        }
        let is_shell = self.mode == Mode::Shell;
        match key.code {
            KeyCode::Char('!') if !ctrl && !is_shell && self.text.is_empty() => {
                self.mode = Mode::Shell;
            }
            KeyCode::Backspace | KeyCode::Esc if is_shell && self.text.is_empty() => {
                self.mode = Mode::Agent;
            }
            KeyCode::Esc if self.text.is_empty() && armed => {
                "/rewind ".clone_into(&mut self.text);
                self.cursor = self.text.len();
            }
            KeyCode::Esc if self.text.is_empty() => self.armed = true,
            KeyCode::Enter if alt => self.insert("\n"),
            KeyCode::Char('j') if ctrl => self.insert("\n"),
            KeyCode::Enter if self.text.trim().is_empty() => {}
            KeyCode::Enter => return Action::Submit,
            KeyCode::Char('c') if ctrl && self.text.is_empty() && is_shell => {
                self.mode = Mode::Agent;
            }
            KeyCode::Char('c') if ctrl => {
                if self.text.is_empty() {
                    return Action::Exit;
                }
                self.text.clear();
                self.cursor = 0;
            }
            KeyCode::Char('d') if ctrl && self.text.is_empty() => return Action::Exit,
            KeyCode::Char('v') if ctrl && !is_shell => return Action::Clipboard,
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Char('u') if ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Tab => {
                if is_shell || !self.complete() {
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
        clusters(&self.text[..self.cursor])
            .last()
            .map_or(0, |(at, _)| *at)
    }

    fn next_boundary(&self) -> usize {
        clusters(&self.text[self.cursor..])
            .first()
            .map_or(self.cursor, |(_, cluster)| self.cursor + cluster.len())
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
        history: &[Past],
        choices: &Choices<'_>,
    ) -> Option<Action> {
        if self.mode == Mode::Shell {
            return None;
        }
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
        let mut items = palette(&self.text, self.cursor, choices);
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
        let selected = pick
            .selected
            .or(preselected(&self.text, self.cursor, count));
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
        self.cursor = item.cursor.unwrap_or(self.text.len());
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

    fn recall(&mut self, history: &[Past], step: isize) {
        let Some(next) = self.recall.checked_add_signed(step) else {
            return;
        };
        if next > history.len() {
            return;
        }
        if self.recall == history.len() {
            self.draft = Past {
                mode: self.mode,
                text: self.text.clone(),
                images: Vec::new(),
            };
        }
        self.recall = next;
        let past = history.get(next).unwrap_or(&self.draft).clone();
        self.mode = past.mode;
        self.text = past.text;
        self.cursor = self.text.len();
        self.recalled = past.images;
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
        let column = shown
            .iter()
            .map(|row| row.width())
            .filter(|width| *width <= LABEL_COLUMN)
            .max()
            .unwrap_or(0);
        shown
            .into_iter()
            .zip(items)
            .enumerate()
            .map(|(index, (row, item))| {
                if !item.about.is_empty() {
                    let pad = " ".repeat(column.saturating_sub(row.width()));
                    return format!("{INDENT}{row}{pad}  {}", item.about);
                }
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
        let command = match self.mode {
            Mode::Agent => command::parse(&self.text).map(|run| run.span),
            Mode::Shell => None,
        };
        let mut rows = Vec::new();
        let mut row = match self.mode {
            Mode::Agent => format!("{}", PROMPT.dim()),
            Mode::Shell => format!("{}", SHELL_PROMPT.red()),
        };
        let mut col = PROMPT.width();
        let mut at_cursor = None;
        for (index, cluster) in clusters(&self.text) {
            if cluster == "\n" {
                if index == self.cursor {
                    at_cursor = Some((rows.len(), col));
                }
                rows.push(mem::replace(&mut row, INDENT.to_owned()));
                col = INDENT.width();
                continue;
            }
            let label = image_of(cluster).map(label);
            let cluster_width = label.as_deref().unwrap_or(cluster).width();
            if col + cluster_width > width {
                rows.push(mem::replace(&mut row, INDENT.to_owned()));
                col = INDENT.width();
            }
            if index == self.cursor {
                at_cursor = Some((rows.len(), col));
            }
            if let Some(label) = &label {
                row.push_str(&label.as_str().cyan().to_string());
            } else if command.as_ref().is_some_and(|span| span.contains(&index)) {
                row.push_str(&cluster.cyan().to_string());
            } else {
                row.push_str(cluster);
            }
            col += cluster_width;
        }
        let (mut cursor_row, mut cursor_col) = at_cursor.unwrap_or((rows.len(), col));
        if self.armed && self.text.is_empty() {
            let hint = fit(REWIND_HINT, width.saturating_sub(col + 1));
            row.push_str(&hint.dim().to_string());
        }
        if cursor_col >= width {
            rows.push(mem::replace(&mut row, INDENT.to_owned()));
            (cursor_row, cursor_col) = (rows.len(), INDENT.width());
        }
        rows.push(row);
        for (index, row_text) in palette.iter().enumerate() {
            if selected == Some(index) {
                let text = row_text.strip_prefix(INDENT).unwrap_or(row_text);
                let shown = cut(text, width - 1 - PROMPT.width());
                rows.push(format!("{}{shown}", PROMPT.dim()));
            } else {
                rows.push(format!("{}", cut(row_text, width - 1).dim()));
            }
        }
        Frame {
            rows,
            cursor: Spot {
                row: cursor_row,
                column: cursor_col,
            },
            images: Vec::new(),
        }
    }
}

fn palette(text: &str, cursor: usize, choices: &Choices<'_>) -> Vec<Item> {
    if let Some((at, typed)) = mention_at(text, cursor) {
        let (before, after) = (&text[..=at], &text[cursor..]);
        return choices
            .mentions(typed)
            .into_iter()
            .take(PALETTE_ROWS)
            .map(|candidate| Item {
                about: String::new(),
                text: format!("{before}{}{after}", candidate.value),
                cursor: Some(before.len() + candidate.value.len()),
                is_final: false,
                label: candidate.label,
                is_current: false,
                efforts: Vec::new(),
                effort: 0,
            })
            .collect();
    }
    let (name, typed) = match command::typing(text) {
        Typing::Name(typed) => {
            let head = &text[..text.len() - typed.len()];
            return command::SPECS
                .iter()
                .filter(|spec| spec.word.starts_with(typed))
                .map(|spec| {
                    let takes_value = spec.name.takes_value();
                    Item {
                        label: format!("/{}", spec.word),
                        about: spec.about.to_owned(),
                        is_current: false,
                        text: if takes_value {
                            format!("{head}{} ", spec.word)
                        } else {
                            format!("{head}{}", spec.word)
                        },
                        cursor: None,
                        is_final: !takes_value,
                        efforts: Vec::new(),
                        effort: 0,
                    }
                })
                .chain(
                    choices
                        .skills
                        .iter()
                        .filter(|skill| skill.for_user && skill.name.starts_with(typed))
                        .map(|skill| Item {
                            label: match &skill.hint {
                                Some(hint) => format!("/{} {hint}", skill.name),
                                None => format!("/{}", skill.name),
                            },
                            about: skill
                                .description
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" "),
                            is_current: false,
                            text: format!("{head}{} ", skill.name),
                            cursor: None,
                            is_final: false,
                            efforts: Vec::new(),
                            effort: 0,
                        }),
                )
                .take(PALETTE_ROWS)
                .collect();
        }
        Typing::Argument(name, typed) => (name, typed),
        Typing::Message => return Vec::new(),
    };
    let head = &text[..text.len() - typed.len()];
    let candidates = match name {
        Name::Model => choices.models(typed),
        Name::Effort => choices.efforts(typed),
        Name::Resume => choices.sessions(typed, now()),
        Name::Rewind => choices.rewinds(typed, now()),
        Name::Mcp => choices.servers(typed),
        Name::Plugin => choices.plugins(typed),
        Name::Compact | Name::Clear | Name::Status | Name::Usage | Name::Help | Name::Exit => {
            Vec::new()
        }
    };
    candidates
        .into_iter()
        .take(PALETTE_ROWS)
        .map(|candidate| Item {
            about: String::new(),
            is_final: !candidate.value.ends_with(['/', ' ']),
            text: format!("{head}{}", candidate.value),
            cursor: None,
            label: candidate.label,
            is_current: candidate.is_current,
            efforts: candidate.efforts,
            effort: candidate.effort,
        })
        .collect()
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn mention_at(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = text.get(..cursor)?;
    let at = before.rfind('@')?;
    let typed = &before[at + 1..];
    let starts_word = before[..at]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    (starts_word && !typed.contains(char::is_whitespace)).then_some((at, typed))
}

fn preselected(text: &str, cursor: usize, count: usize) -> Option<usize> {
    if mention_at(text, cursor).is_some() {
        return (count > 0).then_some(0);
    }
    let has_query = match command::typing(text) {
        Typing::Name(_) | Typing::Argument(Name::Rewind, _) => true,
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

pub(crate) fn echo(input: &Input, columns: usize) -> Vec<String> {
    let line = Line {
        mode: input.mode,
        text: input.text.clone(),
        cursor: input.text.len(),
        ..Line::default()
    };
    line.frame(columns, &[], None).rows
}

const MARKERS: u32 = 0xF_0000;
const MARKER_COUNT: u32 = 0xFFFE;

fn marker(number: usize) -> char {
    let offset = u32::try_from(number).unwrap_or(0) % MARKER_COUNT;
    char::from_u32(MARKERS + offset).unwrap_or(char::REPLACEMENT_CHARACTER)
}

fn image_of_char(c: char) -> Option<usize> {
    let offset = u32::from(c).checked_sub(MARKERS)?;
    (offset < MARKER_COUNT).then(|| usize::try_from(offset).unwrap_or(0))
}

fn image_of(cluster: &str) -> Option<usize> {
    let mut chars = cluster.chars();
    let first = chars.next()?;
    chars
        .next()
        .is_none()
        .then_some(first)
        .and_then(image_of_char)
}

const IMAGE_LABEL: &str = "[Image #";

fn label(number: usize) -> String {
    format!("{IMAGE_LABEL}{number}]")
}

fn images_in(text: &str) -> Vec<usize> {
    let mut numbers = Vec::new();
    for number in text.chars().filter_map(image_of_char) {
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
    numbers
}

fn clusters(text: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    for (at, grapheme) in text.grapheme_indices(true) {
        let mut chars = grapheme.char_indices();
        match chars.next() {
            Some((_, first))
                if image_of_char(first).is_some() && grapheme.len() > first.len_utf8() =>
            {
                let split = first.len_utf8();
                found.push((at, &grapheme[..split]));
                found.push((at + split, &grapheme[split..]));
            }
            _ => found.push((at, grapheme)),
        }
    }
    found
}

fn tile_id(number: usize) -> u8 {
    u8::try_from(number % 255).unwrap_or(0) + 1
}

fn sign(mode: Mode) -> &'static str {
    match mode {
        Mode::Agent => PROMPT,
        Mode::Shell => SHELL_PROMPT,
    }
}

fn width(columns: usize) -> usize {
    columns.max(PROMPT.width() + 2)
}

pub(crate) fn decision(event: &Event, scope: Scope) -> Option<Decision> {
    let Event::Key(key) = event else {
        return None;
    };
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Decision::Refuse);
    }
    Some(match key.code {
        KeyCode::Char('y' | 'Y') => Decision::Once,
        KeyCode::Char('r' | 'R') if scope == Scope::Run => Decision::Run,
        KeyCode::Char('a' | 'A') if scope == Scope::Always => Decision::Always,
        _ => Decision::Refuse,
    })
}

pub(crate) fn interrupts(event: &Event) -> bool {
    matches!(event, Event::Key(key) if key.kind != KeyEventKind::Release && is_interrupt(*key))
}

fn is_interrupt(key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Esc) || ctrl && matches!(key.code, KeyCode::Char('c'))
}

fn cut(text: &str, width: usize) -> String {
    let shown = fit(text, width);
    if shown.len() == text.len() {
        return shown.to_owned();
    }
    format!("{}…", fit(text, width.saturating_sub(1)))
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
        .filter(|c| image_of_char(*c).is_none() && (*c == '\n' || !c.is_control()))
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
    use crate::target::{Current, Files, Offer};

    #[test]
    fn only_y_allows_a_question_and_r_and_a_widen_it_where_offered() {
        let press = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        let answer = |key, scope| decision(&press(KeyCode::Char(key)), scope);

        assert_eq!(answer('y', Scope::Once), Some(Decision::Once));
        assert_eq!(answer('Y', Scope::Run), Some(Decision::Once));
        assert_eq!(answer('r', Scope::Run), Some(Decision::Run));
        assert_eq!(answer('r', Scope::Once), Some(Decision::Refuse));
        assert_eq!(answer('r', Scope::Always), Some(Decision::Refuse));
        assert_eq!(answer('a', Scope::Always), Some(Decision::Always));
        assert_eq!(answer('a', Scope::Run), Some(Decision::Refuse));
        assert_eq!(answer('n', Scope::Run), Some(Decision::Refuse));
        assert_eq!(
            decision(&press(KeyCode::Enter), Scope::Always),
            Some(Decision::Refuse)
        );
        assert_eq!(decision(&Event::Resize(80, 24), Scope::Run), None);
    }

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
        let skills = [goat_core::skill::Skill {
            name: "commit".to_owned(),
            description: "Commit the work\nmore".to_owned(),
            hint: Some("[push|pr|merge]".to_owned()),
            source: goat_core::skill::Source::File(std::path::PathBuf::new()),
            for_model: true,
            for_user: true,
        }];
        let files = Files::listed(
            ["README.md", "src/", "src/input.rs", "src/main.rs"]
                .map(str::to_owned)
                .to_vec(),
        );
        use_choices(&Choices {
            offers: &offers,
            catalog: &catalog,
            sessions: &[],
            sent: &[],
            files: &files,
            skills: &skills,
            servers: &[],
            plugins: &[],
            current: Current {
                account: account.id,
                model: "gpt-6.2-sol",
                effort: None,
            },
        })
    }

    fn message(text: &str) -> Input {
        Input {
            mode: Mode::Agent,
            text: text.to_owned(),
            images: Vec::new(),
        }
    }

    fn shell(text: &str) -> Input {
        Input {
            mode: Mode::Shell,
            text: text.to_owned(),
            images: Vec::new(),
        }
    }

    fn shell_line(text: &str) -> Line {
        Line {
            mode: Mode::Shell,
            ..line(text, text.len())
        }
    }

    fn past(text: &str, mode: Mode) -> Past {
        Past {
            mode,
            text: text.to_owned(),
            images: Vec::new(),
        }
    }

    fn key(line: &mut Line, code: KeyCode, modifiers: KeyModifiers, history: &[Past]) {
        with_choices(|choices| line.key(KeyEvent::new(code, modifiers), history, choices));
    }

    fn rows(text: &str) -> Vec<String> {
        with_choices(|choices| Line::default().rows(&palette(text, text.len(), choices)))
    }

    #[test]
    fn a_label_too_long_for_the_column_does_not_widen_it() {
        let item = |label: &str| Item {
            label: label.to_owned(),
            about: "about".to_owned(),
            is_current: false,
            text: String::new(),
            cursor: None,
            is_final: true,
            efforts: Vec::new(),
            effort: 0,
        };
        let long = format!("/blog {}", "x".repeat(40));

        let rows = Line::default().rows(&[item("/model"), item(&long), item("/exit")]);

        assert_eq!(rows[0], "  /model  about");
        assert_eq!(rows[1], format!("  {long}  about"));
        assert_eq!(rows[2], "  /exit   about");
    }

    #[test]
    fn a_long_row_is_cut_with_an_ellipsis() {
        assert_eq!(
            cut("/skill-author  Writes, revises and tests skills", 24),
            "/skill-author  Writes, …"
        );
        assert_eq!(cut("/exit  Quit", 24), "/exit  Quit");
    }

    #[test]
    fn skills_follow_the_commands_with_their_hint() {
        assert_eq!(
            rows("/co"),
            [
                "  /compact                 Summarize the conversation to free up context",
                "  /commit [push|pr|merge]  Commit the work more",
            ]
        );
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
    fn escape_and_ctrl_c_interrupt_but_other_keys_do_not() {
        let key = |code, modifiers| Event::Key(KeyEvent::new(code, modifiers));

        assert!(interrupts(&key(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(interrupts(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!interrupts(&key(KeyCode::Char('c'), KeyModifiers::NONE)));
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
        let history = [past("first", Mode::Agent), past("second", Mode::Agent)];
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
    fn esc_twice_on_an_empty_prompt_opens_rewind() {
        let mut line = line("", 0);

        press(&mut line, &[KeyCode::Esc]);
        let hint = line.frame(80, &[], None).rows[0].clone();
        press(&mut line, &[KeyCode::Esc]);

        assert!(hint.contains(REWIND_HINT));
        assert_eq!(line.text, "/rewind ");
    }

    #[test]
    fn the_latest_prompt_is_picked_when_rewind_opens() {
        assert_eq!(preselected("/rewind ", 8, 3), Some(0));
        assert_eq!(preselected("/resume ", 8, 3), None);
    }

    #[test]
    fn another_key_between_two_escs_cancels_the_rewind() {
        let mut line = line("", 0);

        press(&mut line, &[KeyCode::Esc, KeyCode::Left, KeyCode::Esc]);

        assert_eq!(line.text, "");
        assert!(line.armed);
    }

    #[test]
    fn a_picked_path_replaces_the_mention_and_keeps_the_rest() {
        let mut line = line("see @ma and more", 7);

        let action = press(&mut line, &[KeyCode::Enter]);

        assert!(matches!(action, Action::Edit));
        assert_eq!(line.text, "see @src/main.rs  and more");
        assert_eq!(line.cursor, "see @src/main.rs ".len());
    }

    #[test]
    fn a_picked_directory_keeps_listing_inside_it() {
        let mut line = line("@sr", 3);

        press(&mut line, &[KeyCode::Tab]);
        let rows = with_choices(|choices| line.rows(&palette(&line.text, line.cursor, choices)));

        assert_eq!(line.text, "@src/");
        assert_eq!(rows, ["  src/input.rs", "  src/main.rs"]);
    }

    #[test]
    fn an_at_sign_inside_a_word_is_not_a_mention() {
        let rows = with_choices(|choices| palette("me@sr", 5, choices).len());

        assert_eq!(rows, 0);
    }

    #[test]
    fn a_rewound_prompt_comes_back_with_its_images() {
        let dir = tempfile::tempdir().unwrap();
        let image = attach::load(&png_in(dir.path())).unwrap().image;
        let mut editor = Editor {
            images: 4,
            ..Editor::default()
        };

        editor.prefill(
            "compare [Image #2] with [Image #2]",
            std::slice::from_ref(&image),
        );

        assert_eq!(
            editor.line.text,
            format!("compare {} with {}", marker(5), marker(5))
        );
        assert_eq!(editor.line.recalled, [(5, image)]);
    }

    #[test]
    fn unqueueing_puts_queued_lines_back_before_the_draft() {
        let mut editor = Editor {
            queue: [message("first"), message("second")].into(),
            line: line("draft", 5),
            ..Editor::default()
        };

        editor.unqueue();

        assert_eq!(editor.line.text, "first\nsecond\ndraft");
        assert!(editor.queue.is_empty());
    }

    #[test]
    fn bang_on_an_empty_line_enters_shell_mode() {
        let mut line = line("", 0);
        press(&mut line, &[KeyCode::Char('!')]);
        assert_eq!((line.mode, line.text.as_str()), (Mode::Shell, ""));
    }

    #[test]
    fn bang_after_text_is_typed() {
        let mut line = line("hi", 2);
        press(&mut line, &[KeyCode::Char('!')]);
        assert_eq!((line.mode, line.text.as_str()), (Mode::Agent, "hi!"));
    }

    #[test]
    fn backspace_on_an_empty_shell_line_returns_to_agent_mode() {
        let mut line = shell_line("");
        press(&mut line, &[KeyCode::Backspace]);
        assert_eq!(line.mode, Mode::Agent);
    }

    #[test]
    fn the_shell_prompt_is_a_red_bang() {
        let rows = shell_line("ls").frame(80, &[], None).rows;
        assert_eq!(rows, [format!("{}ls", SHELL_PROMPT.red())]);
    }

    #[test]
    fn shell_mode_lists_no_commands() {
        let editor = Editor {
            line: shell_line("/mo"),
            ..Editor::default()
        };
        let rows = with_choices(|choices| editor.frame(80, choices).rows);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn a_sent_shell_line_is_queued_as_shell_and_the_prompt_returns_to_agent_mode() {
        let mut editor = Editor {
            line: shell_line("ls"),
            ..Editor::default()
        };

        during_turn(&mut editor, KeyCode::Enter);

        assert_eq!(editor.queue, [shell("ls")]);
        assert_eq!(editor.line.mode, Mode::Agent);
    }

    #[test]
    fn history_recall_brings_back_shell_mode() {
        let history = [past("ls", Mode::Shell)];
        let mut line = Line {
            recall: history.len(),
            ..line("", 0)
        };
        key(&mut line, KeyCode::Up, KeyModifiers::NONE, &history);
        assert_eq!((line.mode, line.text.as_str()), (Mode::Shell, "ls"));
    }

    #[test]
    fn unqueueing_shell_lines_only_stays_in_shell_mode() {
        let mut editor = Editor {
            queue: [shell("ls"), shell("pwd")].into(),
            ..Editor::default()
        };

        editor.unqueue();

        assert_eq!(
            (editor.line.mode, editor.line.text.as_str()),
            (Mode::Shell, "ls\npwd")
        );
    }

    #[test]
    fn unqueueing_mixed_lines_keeps_shell_lines_as_text() {
        let mut editor = Editor {
            queue: [shell("ls"), message("fix it")].into(),
            ..Editor::default()
        };

        editor.unqueue();

        assert_eq!(
            (editor.line.mode, editor.line.text.as_str()),
            (Mode::Agent, "!ls\nfix it")
        );
    }

    fn png_in(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("shot.png");
        ::image::RgbImage::new(4, 2).save(&path).unwrap();
        path
    }

    fn pasting(editor: &mut Editor, text: &str) {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        editor.paste(text, &mut terminal);
    }

    #[test]
    fn a_pasted_image_path_becomes_an_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let path = png_in(dir.path());
        let mut editor = Editor {
            line: line("see ", 4),
            ..Editor::default()
        };

        pasting(&mut editor, &format!("{} ", path.display()));
        let input = editor.take();

        assert_eq!(input.plain(), "see [Image #1] ");
        assert_eq!(input.images.len(), 1);
    }

    #[test]
    fn deleting_the_token_drops_the_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = png_in(dir.path());
        let mut editor = Editor::default();
        pasting(&mut editor, &path.display().to_string());

        editor.line.text = "never mind".to_owned();
        editor.line.cursor = editor.line.text.len();
        let input = editor.take();

        assert_eq!(input.images, Vec::new());
    }

    #[test]
    fn a_path_pasted_in_shell_mode_stays_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = png_in(dir.path());
        let mut editor = Editor {
            line: shell_line(""),
            ..Editor::default()
        };

        pasting(&mut editor, &path.display().to_string());

        assert_eq!(editor.line.text, path.display().to_string());
        assert_eq!(editor.line.attachments.len(), 0);
    }

    #[test]
    fn thumbnails_sit_above_the_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = png_in(dir.path());
        let mut editor = Editor::default();
        pasting(&mut editor, &path.display().to_string());
        editor.line.attachments[0].tile = Some(Tile { id: 2, columns: 3 });

        let frame = with_choices(|choices| editor.frame(40, choices));

        assert_eq!(frame.rows.len(), usize::from(kitty::ROWS) + 1);
        assert_eq!(
            frame.images,
            [Placement {
                row: 0,
                column: 2,
                id: 2,
                columns: 3
            }]
        );
        assert_eq!(frame.cursor.row, usize::from(kitty::ROWS));
    }

    #[test]
    fn unqueueing_brings_the_attachments_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = png_in(dir.path());
        let mut editor = Editor::default();
        pasting(&mut editor, &path.display().to_string());
        let input = editor.take();
        editor.queue.push_back(input);

        editor.unqueue();
        let again = editor.take();

        assert_eq!(again.plain(), "[Image #1] ");
        assert_eq!(again.images.len(), 1);
    }

    fn with_image(dir: &std::path::Path) -> Editor {
        let mut editor = Editor {
            line: line("look ", 5),
            store: Some(Store::open_in_memory().unwrap()),
            ..Editor::default()
        };
        pasting(&mut editor, &png_in(dir).display().to_string());
        editor
    }

    #[test]
    fn backspace_after_an_image_removes_it_whole() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = with_image(dir.path());

        press(&mut editor.line, &[KeyCode::Backspace, KeyCode::Backspace]);
        let input = editor.take();

        assert_eq!(input.plain(), "look ");
        assert_eq!(input.images, Vec::new());
    }

    #[test]
    fn arrows_step_over_an_image_in_one_move() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = with_image(dir.path());

        press(&mut editor.line, &[KeyCode::Left, KeyCode::Left]);

        assert_eq!(editor.line.cursor, "look ".len());
    }

    #[test]
    fn an_image_shows_as_its_label() {
        let dir = tempfile::tempdir().unwrap();
        let editor = with_image(dir.path());

        let rows = editor.line.frame(80, &[], None).rows;

        assert_eq!(
            rows,
            [format!("{}look {} ", PROMPT.dim(), "[Image #1]".cyan())]
        );
    }

    #[test]
    fn a_mark_typed_after_an_image_stays_separate() {
        let text = format!("{}\u{301}", marker(1));

        let found = clusters(&text);

        assert_eq!(found.len(), 2);
        assert_eq!(image_of(found[0].1), Some(1));
    }

    #[test]
    fn pasted_text_cannot_carry_image_markers() {
        assert_eq!(clean_paste(&format!("a{}b", marker(3))), "ab");
    }

    #[test]
    fn recalling_a_sent_message_brings_its_image_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = with_image(dir.path());
        editor.take();

        with_choices(|choices| {
            editor.line.key(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                &editor.history,
                choices,
            )
        });
        editor.restore();
        let again = editor.take();

        assert_eq!(again.plain(), "look [Image #1] ");
        assert_eq!(again.images.len(), 1);
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
            ["  /effort  Set the reasoning effort", "  /exit    Quit"]
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
        let mut keys = vec![KeyCode::Down; 10];
        keys.push(KeyCode::Enter);
        let action = press(&mut line, &keys);
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
        let history = [past("first", Mode::Agent)];
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
        let rows = with_choices(|choices| line.rows(&palette(&line.text, line.cursor, choices)));
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

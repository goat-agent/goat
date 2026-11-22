use std::io::{self, BufWriter, Stdout, Write};
use std::mem;
use std::time::{Duration, Instant};

use crossterm::style::{Color, Stylize};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, queue};
use goat_core::message::{Diff, Sign};
use goat_core::session::{Event, Sink};
use goat_core::tool::{self, Kind};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::highlight;
use crate::input::{Input, Mode, echo, fit};
use crate::kitty::{self, Cell};
use crate::markdown::{Markdown, Style};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INDENT: &str = "  ";
const TAB: &str = "    ";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const GOAT: [&str; 3] = ["▀▀▄▄      ▄▄▀▀", "  ▄▄▀████▀▄▄  ", "     ▀██▀     "];
const GAP: &str = "   ";
const CLEAR: &str = "\x1b[H\x1b[2J\x1b[3J";
const PRIMARY: Color = Color::AnsiValue(141);

pub(crate) struct Terminal<W: Write = BufWriter<Stdout>> {
    out: W,
    columns: fn() -> usize,
    pending: String,
    composer: Option<Frame>,
    graphics: Option<Cell>,
    placed: Vec<u8>,
    live: Option<usize>,
    is_updating: bool,
    has_output: bool,
    section: Option<Section>,
    markdown: Markdown,
    code: Option<highlight::Block>,
    calls: usize,
    activity: Option<Activity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Shell,
    Reply,
    Note,
    Reasoning,
    Text,
    Run,
}

struct Activity {
    work: Work,
    started: Instant,
    frame: usize,
    retrying: Option<u32>,
}

#[derive(Clone, Copy)]
pub(crate) enum Work {
    Turn,
    Shell,
    Compact,
}

pub(crate) struct Frame {
    pub(crate) rows: Vec<String>,
    pub(crate) cursor: Spot,
    pub(crate) images: Vec<Placement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    pub(crate) row: usize,
    pub(crate) column: u16,
    pub(crate) id: u8,
    pub(crate) columns: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Spot {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

pub(crate) struct Banner<'a> {
    pub(crate) model: &'a str,
    pub(crate) directory: &'a str,
}

pub(crate) enum Ending {
    Complete,
    Interrupted,
    Failed(String),
}

impl Terminal {
    pub(crate) fn new() -> Self {
        Self::with_writer(BufWriter::new(io::stdout()), terminal_columns)
    }
}

impl<W: Write> Terminal<W> {
    pub(crate) fn with_writer(out: W, columns: fn() -> usize) -> Self {
        Self {
            out,
            columns,
            pending: String::new(),
            composer: None,
            graphics: None,
            placed: Vec::new(),
            live: None,
            is_updating: false,
            has_output: false,
            section: None,
            markdown: Markdown::default(),
            code: None,
            calls: 0,
            activity: None,
        }
    }

    pub(crate) fn columns(&self) -> usize {
        (self.columns)()
    }

    pub(crate) fn graphics(&self) -> Option<Cell> {
        self.graphics
    }

    pub(crate) fn set_graphics(&mut self, cell: Option<Cell>) {
        self.graphics = cell;
    }

    pub(crate) fn write_raw(&mut self, text: &str) {
        self.out.write_all(text.as_bytes()).ok();
    }

    pub(crate) fn show(&mut self, composer: Frame) {
        self.erase();
        self.composer = Some(composer);
        self.flush();
    }

    pub(crate) fn echo(&mut self, rows: &[String]) {
        self.erase();
        for row in rows {
            write!(self.out, "{row}\r\n").ok();
        }
        self.has_output = true;
        self.flush();
    }

    pub(crate) fn close(&mut self) {
        self.erase();
        self.composer = None;
        self.flush();
    }

    pub(crate) fn clear(&mut self) {
        self.erase();
        self.pending.clear();
        self.markdown = Markdown::default();
        self.section = None;
        self.has_output = false;
        self.calls = 0;
        write!(self.out, "{CLEAR}").ok();
        self.flush();
    }

    pub(crate) fn banner(&mut self, banner: &Banner<'_>) {
        self.enter(Section::Note);
        let room = (self.columns)().saturating_sub(GOAT[0].chars().count() + GAP.len());
        let lines = [
            format!("{} {}", "Goat".bold(), format!("v{VERSION}").dim()),
            format!("{}", shorten(banner.model, room).dim()),
            format!("{}", shorten(banner.directory, room).dim()),
        ];
        for (art, line) in GOAT.iter().zip(lines) {
            self.put(&format!("{}{GAP}{line}\n", art.with(PRIMARY)));
        }
        self.flush();
    }

    pub(crate) fn farewell(&mut self, command: &str) {
        self.put(&format!(
            "{}\n{INDENT}{}\n",
            "Resume this session with:".dim(),
            command.cyan()
        ));
        self.flush();
    }

    pub(crate) fn note(&mut self, text: &str) {
        self.enter(Section::Note);
        for line in text.lines() {
            self.put(&format!("{INDENT}{}\n", line.dim()));
        }
        self.flush();
    }

    pub(crate) fn reply(&mut self, text: &str) {
        self.enter(Section::Reply);
        for line in text.lines() {
            self.put(&format!("{INDENT}{}\n", line.dim()));
        }
        self.flush();
    }

    pub(crate) fn lines(&mut self, lines: &[String]) {
        self.enter(Section::Reply);
        for line in lines {
            self.put(&format!("{INDENT}{line}\n"));
        }
        self.flush();
    }

    pub(crate) fn outcome(&mut self, outcome: Result<&str, &str>) {
        self.enter(Section::Reply);
        let text = match outcome {
            Ok(text) | Err(text) => text,
        };
        for (index, line) in text.lines().enumerate() {
            let branch = if index == 0 { "└" } else { " " };
            let line = match outcome {
                Ok(_) => line.to_owned(),
                Err(_) => format!("{}", line.red()),
            };
            self.put(&format!("{INDENT}{} {line}\n", branch.dim()));
        }
        self.flush();
    }

    pub(crate) fn error(&mut self, text: &str) {
        self.enter(Section::Note);
        for line in text.lines() {
            self.put(&format!("{INDENT}{}\n", line.red()));
        }
        self.flush();
    }

    pub(crate) fn settle(&mut self) {
        self.close_section();
        if self.section.take().is_some() {
            self.put("\n");
        }
        self.flush();
        self.has_output = true;
    }

    pub(crate) fn begin_turn(&mut self, work: Work) {
        self.erase();
        self.activity = Some(Activity {
            work,
            started: Instant::now(),
            frame: 0,
            retrying: None,
        });
        self.flush();
    }

    pub(crate) fn tick(&mut self) {
        let Some(activity) = &mut self.activity else {
            return;
        };
        activity.frame = (activity.frame + 1) % SPINNER.len();
        self.erase();
        self.flush();
    }

    pub(crate) fn finish_turn(&mut self, ending: Ending) {
        self.close_section();
        self.erase();
        self.activity = None;
        match ending {
            Ending::Complete => {}
            Ending::Interrupted => self.error("Interrupted"),
            Ending::Failed(problem) => self.error(&problem),
        }
        self.flush();
    }

    fn enter(&mut self, section: Section) {
        if self.section != Some(section) {
            self.open(section);
        }
    }

    fn open(&mut self, section: Section) {
        self.close_section();
        if self.has_output && section != Section::Reply {
            self.put("\n");
        }
        self.section = Some(section);
        self.calls = 0;
    }

    fn close_section(&mut self) {
        if let Some(section @ (Section::Reasoning | Section::Text)) = self.section {
            let mut markdown = mem::take(&mut self.markdown);
            markdown.finish(&mut |style, text| self.paint(section, style, text));
            self.code = None;
        }
        if !self.pending.is_empty() {
            self.put("\n");
        }
    }

    fn stream(&mut self, section: Section, chunk: &str) {
        self.enter(section);
        let mut markdown = mem::take(&mut self.markdown);
        markdown.push(chunk, &mut |style, text| self.paint(section, style, text));
        self.markdown = markdown;
    }

    fn paint(&mut self, section: Section, style: Style, text: &str) {
        if section != Section::Reasoning {
            match style {
                Style::Plain => self.put(text),
                Style::Strong | Style::Heading => self.put(&format!("{}", text.bold())),
                Style::Code => self.put(&format!("{}", text.cyan())),
                Style::Marker => self.put(&format!("{}", text.dim())),
                Style::Open => self.code = Some(highlight::Block::new(text)),
                Style::Close => self.code = None,
                Style::Block => {
                    let painted = match &mut self.code {
                        Some(block) => block.paint(text),
                        None => text.to_owned(),
                    };
                    self.put(&painted);
                }
            }
            return;
        }
        if matches!(style, Style::Open | Style::Close) {
            return;
        }
        if text == "\n" {
            if !self.pending.is_empty() {
                self.put("\n");
            }
            return;
        }
        if self.pending.is_empty() {
            self.put(INDENT);
        }
        match style {
            Style::Strong | Style::Heading => self.put(&format!("{}", text.dim().bold())),
            _ => self.put(&format!("{}", text.dim())),
        }
    }

    fn call(&mut self, call: &tool::Call) {
        let verb = match call.kind {
            Kind::Bash => "Bash",
            Kind::Read => "Read",
            Kind::Edit => "Edit",
            Kind::Write => "Write",
            Kind::Fetch => "WebFetch",
            Kind::Search => "WebSearch",
            Kind::Skill => "Skill",
            Kind::Mcp => "Mcp",
        };
        self.call_line(verb, &call.target);
    }

    fn call_line(&mut self, verb: &str, target: &str) {
        self.enter(Section::Run);
        let branch = if self.calls == 0 { "└" } else { " " };
        self.calls += 1;
        let mut lines = target.lines();
        let mut target = lines.next().unwrap_or_default().to_owned();
        if lines.next().is_some() {
            target.push_str(" …");
        }
        let room = (self.columns)().saturating_sub(verb.len() + 7);
        self.put(&format!(
            "{INDENT}{} {verb} {}\n",
            branch.dim(),
            shorten(&target, room).dim()
        ));
    }

    fn changed(&mut self, diff: &Diff) {
        self.enter(Section::Run);
        let gutter = diff
            .lines
            .iter()
            .map(|line| line.number.to_string().len())
            .max()
            .unwrap_or(1);
        let lead = format!("{INDENT}{INDENT}");
        self.put(&format!(
            "{lead}{} {}\n",
            format!("+{}", diff.added).green(),
            format!("-{}", diff.removed).red()
        ));
        let room = (self.columns)().saturating_sub(lead.len() + gutter + 3);
        for line in &diff.lines {
            let text = shorten(&printable(&line.text.replace('\t', TAB)), room);
            let number = format!("{:>gutter$}", line.number);
            let shown = match line.sign {
                Sign::Same => format!("{}", format!("{number}   {text}").dim()),
                Sign::Added => format!("{}", format!("{number} + {text}").green()),
                Sign::Removed => format!("{}", format!("{number} - {text}").red()),
                Sign::Gap => format!("{}", format!("{:>gutter$}", "⋮").dim()),
            };
            self.put(&format!("{lead}{shown}\n"));
        }
        if diff.hidden > 0 {
            self.put(&format!(
                "{lead}{}\n",
                format!("… {} more lines", diff.hidden).dim()
            ));
        }
    }

    fn prompt(&mut self, mode: Mode, text: &str) {
        self.settle();
        let input = Input {
            mode,
            text: text.to_owned(),
            images: Vec::new(),
        };
        let rows = echo(&input, self.columns());
        self.echo(&rows);
    }

    fn shell_output(&mut self, chunk: &str) {
        self.enter(Section::Shell);
        for piece in printable(chunk).split_inclusive('\n') {
            if self.pending.is_empty() {
                self.put(INDENT);
            }
            self.put(piece);
        }
    }

    fn shell_finished(&mut self, code: Option<i32>, problem: Option<&str>) {
        self.enter(Section::Shell);
        if !self.pending.is_empty() {
            self.put("\n");
        }
        let status = match (problem, code) {
            (Some(problem), _) => format!("{}", problem.red()),
            (None, Some(0)) => format!("{}", "exit 0".dim()),
            (None, Some(code)) => format!("{}", format!("exit {code}").red()),
            (None, None) => format!("{}", "killed".red()),
        };
        self.put(&format!("{INDENT}{} {status}\n", "└".dim()));
    }

    fn put(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.erase();
        self.has_output = true;
        self.pending.push_str(text);
        let Some(end) = self.pending.rfind('\n') else {
            return;
        };
        let rest = self.pending.split_off(end + 1);
        let lines = mem::replace(&mut self.pending, rest);
        write!(self.out, "{}", lines.replace('\n', "\r\n")).ok();
    }

    fn erase(&mut self) {
        let Some(row) = self.live.take() else {
            return;
        };
        if !self.is_updating {
            queue!(self.out, terminal::BeginSynchronizedUpdate).ok();
            self.is_updating = true;
        }
        for id in mem::take(&mut self.placed) {
            write!(self.out, "{}", kitty::remove(id)).ok();
        }
        queue!(self.out, cursor::MoveToColumn(0)).ok();
        if row > 0 {
            queue!(self.out, cursor::MoveUp(cells(row))).ok();
        }
        queue!(self.out, terminal::Clear(ClearType::FromCursorDown)).ok();
    }

    fn draw(&mut self) {
        let wrapped = wrap(&self.pending, (self.columns)());
        let spinner = self.activity.as_ref().map(|activity| {
            let took = format!("({})", elapsed(activity.started.elapsed()));
            let doing = match (activity.retrying, activity.work) {
                (Some(attempt), _) => format!("Retrying (attempt {attempt}), Esc to stop"),
                (None, Work::Turn) => "Working".to_owned(),
                (None, Work::Shell) => "Running".to_owned(),
                (None, Work::Compact) => "Compacting".to_owned(),
            };
            format!("{} {doing} {}", SPINNER[activity.frame], took.dim())
        });
        let above = wrapped.len() + usize::from(spinner.is_some());
        let composer = self.composer.iter().flat_map(|frame| &frame.rows);
        let shown = wrapped
            .iter()
            .chain(&spinner)
            .chain(composer)
            .map(String::as_str);
        let images = self.composer.iter().flat_map(|frame| &frame.images);
        let mut count = 0;
        for (index, row) in shown.enumerate() {
            if index > 0 {
                write!(self.out, "\r\n").ok();
            }
            for image in images.clone().filter(|image| above + image.row == index) {
                queue!(self.out, cursor::MoveToColumn(image.column)).ok();
                write!(self.out, "{}", kitty::place(image.id, image.columns)).ok();
                queue!(self.out, cursor::MoveToColumn(0)).ok();
                self.placed.push(image.id);
            }
            write!(self.out, "{row}").ok();
            count += 1;
        }
        if count > 0 {
            let last = count - 1;
            let row = match &self.composer {
                Some(frame) => {
                    let row = above + frame.cursor.row;
                    if last > row {
                        queue!(self.out, cursor::MoveUp(cells(last - row))).ok();
                    }
                    queue!(self.out, cursor::MoveToColumn(cells(frame.cursor.column))).ok();
                    row
                }
                None => last,
            };
            self.live = Some(row);
        }
        if mem::take(&mut self.is_updating) {
            queue!(self.out, terminal::EndSynchronizedUpdate).ok();
        }
    }

    fn flush(&mut self) {
        if self.live.is_none() {
            self.draw();
        }
        self.out.flush().ok();
    }
}

impl<W: Write> Sink for Terminal<W> {
    fn emit(&mut self, event: Event<'_>) {
        if let Some(activity) = &mut self.activity {
            activity.retrying = match event {
                Event::Retrying { attempt, .. } => Some(attempt),
                Event::Usage(_) => activity.retrying,
                _ => None,
            };
        }
        match event {
            Event::Prompt(text) => self.prompt(Mode::Agent, text),
            Event::ShellCommand(command) => self.prompt(Mode::Shell, command),
            Event::Text(text) => self.stream(Section::Text, text),
            Event::Reasoning(chunk) => self.stream(Section::Reasoning, chunk),
            Event::RunStarted { description } => {
                self.open(Section::Run);
                let description = match first_line(description) {
                    "" => "Running a script",
                    line => line,
                };
                self.put(&format!("{} {description}\n", "•".dim()));
            }
            Event::Call(call) => self.call(call),
            Event::Changed(diff) => self.changed(diff),
            Event::Loaded { path } => self.call_line("Loaded", path),
            Event::Untracked { reason } => {
                self.close_section();
                self.note(&format!(
                    "/rewind cannot restore files here, because {reason}"
                ));
            }
            Event::CallFailed { problem } => {
                self.enter(Section::Run);
                self.put(&format!("{INDENT}{INDENT}{}\n", first_line(problem).red()));
            }
            Event::RunFinished { outcome } if outcome.is_error => {
                self.enter(Section::Run);
                self.put(&format!("{INDENT}{}\n", first_line(&outcome.output).red()));
            }
            Event::Truncated => {
                self.close_section();
                self.note("The reply hit the output limit and was cut off");
            }
            Event::Refused => {
                self.close_section();
                self.error(
                    "The model declined to answer; rephrase, or try another model with /model",
                );
            }
            Event::ShellOutput(chunk) => self.shell_output(chunk),
            Event::ShellFinished { code, problem } => self.shell_finished(code, problem),
            Event::Retrying { restarts: true, .. } => {
                self.close_section();
                self.note("The reply was cut off, so it starts over");
            }
            Event::RunFinished { .. } | Event::Usage(_) | Event::Retrying { .. } => {}
            Event::Compacting => {
                self.open(Section::Note);
                self.put(&format!(
                    "{INDENT}{}\n",
                    "Compacting the conversation".dim()
                ));
            }
        }
        self.flush();
    }
}

fn terminal_columns() -> usize {
    terminal::size().map_or(80, |(columns, _)| usize::from(columns))
}

fn wrap(text: &str, columns: usize) -> Vec<String> {
    let mut rows = Vec::new();
    if text.is_empty() {
        return rows;
    }
    let mut row = String::new();
    let mut used = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let piece = escape(rest)
            .or_else(|| rest.graphemes(true).next())
            .unwrap_or(rest);
        rest = &rest[piece.len()..];
        let width = if piece.starts_with('\x1b') {
            0
        } else {
            piece.width()
        };
        if used > 0 && used + width > columns {
            rows.push(mem::take(&mut row));
            used = 0;
        }
        row.push_str(piece);
        used += width;
    }
    rows.push(row);
    rows
}

fn printable(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '\x1b' {
            let skip = escape(rest).map_or_else(
                || 1 + rest[1..].chars().next().map_or(0, char::len_utf8),
                str::len,
            );
            rest = &rest[skip..];
            continue;
        }
        match c {
            '\n' => shown.push('\n'),
            '\t' => shown.push_str(TAB),
            c if c.is_control() => {}
            c => shown.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
    shown
}

fn escape(text: &str) -> Option<&str> {
    let body = text.strip_prefix("\x1b[")?;
    let end = body.find(|c: char| ('@'..='~').contains(&c))?;
    Some(&text[..=2 + end])
}

fn cells(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

fn shorten(text: &str, width: usize) -> String {
    let shown = fit(text, width);
    if shown.len() < text.len() {
        format!("{shown}…")
    } else {
        shown.to_owned()
    }
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
}

fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        return format!("{seconds}s");
    }
    format!("{}m {:02}s", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use goat_core::message::DiffLine;
    use goat_core::tool::{Call, Outcome};

    use super::*;

    fn screen(events: &[Event<'_>]) -> String {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        for event in events {
            terminal.emit(*event);
        }
        terminal.settle();
        visible(&terminal.out)
    }

    fn visible(out: &[u8]) -> String {
        let text = String::from_utf8(out.to_vec()).unwrap();
        let mut rows = vec![String::new()];
        let mut row = 0;
        let mut rest = text.as_str();
        while let Some(c) = rest.chars().next() {
            if let Some(graphics) = rest.strip_prefix("\x1b_") {
                let end = graphics.find("\x1b\\").unwrap();
                rest = &graphics[end + 2..];
                continue;
            }
            if let Some(code) = escape(rest) {
                let kind = code.chars().last().unwrap();
                let count = code[2..code.len() - 1].parse::<usize>().unwrap_or(1);
                match kind {
                    'A' => row -= count,
                    'J' => {
                        rows.truncate(row + 1);
                        rows[row].clear();
                    }
                    'm' => rows[row].push_str(code),
                    _ => {}
                }
                rest = &rest[code.len()..];
                continue;
            }
            match c {
                '\r' => {}
                '\n' => {
                    row += 1;
                    if rows.len() == row {
                        rows.push(String::new());
                    }
                }
                _ => rows[row].push(c),
            }
            rest = &rest[c.len_utf8()..];
        }
        rows.join("\n")
    }

    fn composer(text: &str) -> Frame {
        Frame {
            rows: vec![format!("› {text}")],
            cursor: Spot {
                row: 0,
                column: 2 + text.len(),
            },
            images: Vec::new(),
        }
    }

    fn call(kind: Kind, target: &str) -> Call {
        Call {
            kind,
            target: target.into(),
        }
    }

    #[test]
    fn a_change_shows_its_counts_and_lines_under_the_call() {
        let edit = call(Kind::Edit, "src/a.rs");
        let diff = Diff {
            added: 1,
            removed: 1,
            lines: vec![
                DiffLine {
                    sign: Sign::Same,
                    number: 9,
                    text: "fn a() {".into(),
                },
                DiffLine {
                    sign: Sign::Removed,
                    number: 10,
                    text: "    old();".into(),
                },
                DiffLine {
                    sign: Sign::Added,
                    number: 10,
                    text: "    new();".into(),
                },
            ],
            hidden: 3,
        };

        let screen = screen(&[Event::Call(&edit), Event::Changed(&diff)]);

        let expected = [
            format!("  {} Edit {}", "└".dim(), "src/a.rs".dim()),
            format!("    {} {}", "+1".green(), "-1".red()),
            format!("    {}", " 9   fn a() {".dim()),
            format!("    {}", "10 -     old();".red()),
            format!("    {}", "10 +     new();".green()),
            format!("    {}", "… 3 more lines".dim()),
        ];
        assert!(screen.contains(&expected.join("\n")), "{screen:?}");
    }

    #[test]
    fn a_retry_shows_on_the_spinner_and_a_cut_reply_says_it_starts_over() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 60);
        terminal.begin_turn(Work::Turn);
        terminal.emit(Event::Text("Partial"));

        terminal.emit(Event::Retrying {
            attempt: 2,
            restarts: true,
        });
        let during = visible(&terminal.out);
        terminal.emit(Event::Text("Done."));
        let after = visible(&terminal.out);

        assert!(during.contains("Retrying (attempt 2), Esc to stop"));
        assert!(during.contains("The reply was cut off, so it starts over"));
        assert!(!after.contains("Retrying"));
    }

    #[test]
    fn a_code_block_shows_colored_code_without_its_fences() {
        let screen = screen(&[Event::Text("Run:\n```rust\nlet a = 1;\n```\nDone.")]);

        assert!(!screen.contains("```"), "{screen:?}");
        assert!(screen.contains(&"let".magenta().to_string()));
        assert!(screen.contains("Run:\n"));
        assert!(screen.contains("Done."));
    }

    #[test]
    fn clearing_wipes_the_screen_and_starts_without_a_gap() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.emit(Event::Text("Done."));
        terminal.settle();

        terminal.clear();
        terminal.note("Fresh");

        let out = String::from_utf8(terminal.out.clone()).unwrap();
        let (_, after) = out.rsplit_once(CLEAR).unwrap();
        assert_eq!(visible(after.as_bytes()), format!("  {}\n", "Fresh".dim()));
    }

    #[test]
    fn reasoning_streams_dimmed_and_text_follows_after_a_blank_line() {
        let screen = screen(&[
            Event::Reasoning("**Checking"),
            Event::Reasoning(" tests**\n"),
            Event::Reasoning("Looking"),
            Event::Reasoning(" around\n"),
            Event::Text("Done."),
        ]);

        let expected = format!(
            "  {}{}\n  {}{}\n\nDone.\n\n",
            "Checking".dim().bold(),
            " tests".dim().bold(),
            "Looking".dim(),
            " around".dim()
        );
        assert_eq!(screen, expected);
    }

    #[test]
    fn calls_hang_under_their_run_with_a_capitalized_verb() {
        let read = call(Kind::Read, "Cargo.toml");
        let bash = call(Kind::Bash, "cargo test\ncargo clippy");
        let screen = screen(&[
            Event::RunStarted {
                description: "Checking the build",
            },
            Event::Call(&read),
            Event::Call(&bash),
        ]);

        let expected = format!(
            "{} Checking the build\n  {} Read {}\n  {} Bash {}\n\n",
            "•".dim(),
            "└".dim(),
            "Cargo.toml".dim(),
            " ".dim(),
            "cargo test …".dim()
        );
        assert_eq!(screen, expected);
    }

    #[test]
    fn web_calls_show_their_function_names() {
        let search = call(Kind::Search, "tokio");
        let fetch = call(Kind::Fetch, "https://tokio.rs");
        let screen = screen(&[Event::Call(&search), Event::Call(&fetch)]);

        let expected = format!(
            "  {} WebSearch {}\n  {} WebFetch {}\n\n",
            "└".dim(),
            "tokio".dim(),
            " ".dim(),
            "https://tokio.rs".dim()
        );
        assert_eq!(screen, expected);
    }

    #[test]
    fn long_call_targets_are_cut_to_the_width() {
        let bash = call(Kind::Bash, &"x".repeat(100));
        let screen = screen(&[Event::Call(&bash)]);

        let expected = format!(
            "  {} Bash {}\n\n",
            "└".dim(),
            format!("{}…", "x".repeat(29)).dim()
        );
        assert_eq!(screen, expected);
    }

    #[test]
    fn a_failed_run_shows_the_first_line_of_its_output() {
        let outcome = Outcome {
            output: "\nSyntaxError: unexpected token\n  at line 3".into(),
            is_error: true,
            images: Vec::new(),
        };
        let screen = screen(&[Event::RunFinished { outcome: &outcome }]);

        let expected = format!("  {}\n\n", "SyntaxError: unexpected token".red());
        assert_eq!(screen, expected);
    }

    #[test]
    fn markdown_text_is_styled_while_streaming() {
        let screen = screen(&[Event::Text("Run `cargo"), Event::Text(" test`\n")]);

        let expected = format!("Run {}{}\n\n", "cargo".cyan(), " test".cyan());
        assert_eq!(screen, expected);
    }

    #[test]
    fn output_after_a_prompt_starts_after_a_blank_line() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.settle();
        terminal.note("Switched to gpt-6.2-sol");

        let expected = format!("\n  {}\n", "Switched to gpt-6.2-sol".dim());
        assert_eq!(visible(&terminal.out), expected);
    }

    #[test]
    fn a_command_result_hangs_under_the_command() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.settle();
        terminal.outcome(Ok("Set effort to high"));
        terminal.outcome(Err("no account offers gpt-6.3\nretry later"));

        let expected = format!(
            "  {} Set effort to high\n  {} {}\n  {} {}\n",
            "└".dim(),
            "└".dim(),
            "no account offers gpt-6.3".red(),
            " ".dim(),
            "retry later".red()
        );
        assert_eq!(visible(&terminal.out), expected);
    }

    #[test]
    fn the_banner_puts_the_version_model_and_directory_beside_the_goat() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.banner(&Banner {
            model: "GPT-6.2 Sol on openai:codex with high effort",
            directory: "~/goat",
        });
        terminal.settle();
        terminal.show(composer(""));

        let expected = format!(
            "{}   {} {}\n{}   {}\n{}   {}\n\n› ",
            GOAT[0].with(PRIMARY),
            "Goat".bold(),
            format!("v{VERSION}").dim(),
            GOAT[1].with(PRIMARY),
            "GPT-6.2 Sol on openai:c…".dim(),
            GOAT[2].with(PRIMARY),
            "~/goat".dim()
        );
        assert_eq!(visible(&terminal.out), expected);
    }

    #[test]
    fn farewell_shows_how_to_resume_below_the_transcript() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.emit(Event::Text("Done.\n"));
        terminal.settle();
        terminal.show(composer(""));
        terminal.close();
        terminal.farewell("goat --resume 0193");

        let expected = format!(
            "Done.\n\n{}\n  {}\n",
            "Resume this session with:".dim(),
            "goat --resume 0193".cyan()
        );
        assert_eq!(visible(&terminal.out), expected);
    }

    #[test]
    fn the_prompt_stays_below_streaming_output() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer("next"));
        terminal.begin_turn(Work::Turn);
        terminal.emit(Event::Text("Hello\nwor"));

        let expected = format!("Hello\nwor\n⠋ Working {}\n› next", "(0s)".dim());
        assert_eq!(visible(&terminal.out), expected);
    }

    #[test]
    fn a_finished_turn_leaves_only_the_prompt_below() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer(""));
        terminal.begin_turn(Work::Turn);
        terminal.emit(Event::Text("Done."));
        terminal.finish_turn(Ending::Complete);
        terminal.settle();

        assert_eq!(visible(&terminal.out), "Done.\n\n› ");
    }

    #[test]
    fn the_prompt_keeps_its_row_when_a_turn_ends() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer(""));
        terminal.begin_turn(Work::Turn);
        terminal.emit(Event::Text("Done.\n"));
        let working = visible(&terminal.out).lines().count();

        terminal.finish_turn(Ending::Complete);
        terminal.settle();

        assert_eq!(visible(&terminal.out).lines().count(), working);
    }

    #[test]
    fn an_echo_goes_above_the_prompt() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer("hi"));
        terminal.echo(&["› hi".to_owned()]);
        terminal.show(composer(""));

        assert_eq!(visible(&terminal.out), "› hi\n› ");
    }

    fn with_thumbnail() -> Frame {
        Frame {
            rows: vec![String::new(), "› ".to_owned()],
            cursor: Spot { row: 1, column: 2 },
            images: vec![Placement {
                row: 0,
                column: 2,
                id: 9,
                columns: 6,
            }],
        }
    }

    #[test]
    fn a_thumbnail_is_placed_and_moved_on_each_draw() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(with_thumbnail());
        terminal.show(with_thumbnail());
        terminal.close();

        let out = String::from_utf8(terminal.out).unwrap();
        assert_eq!(out.matches(&kitty::place(9, 6)).count(), 2);
        assert_eq!(out.matches(&kitty::remove(9)).count(), 2);
        assert_eq!(visible(out.as_bytes()), "");
    }

    #[test]
    fn closing_clears_the_prompt() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer("hi"));
        terminal.close();

        assert_eq!(visible(&terminal.out), "");
    }

    #[test]
    fn a_partial_line_wraps_at_the_width_and_keeps_its_style() {
        let text = format!("abc{}", "defg".dim());

        let rows = wrap(&text, 5);

        assert_eq!(rows, ["abc\u{1b}[2mde", "fg\u{1b}[0m"]);
    }

    #[test]
    fn shell_output_is_indented_cleaned_and_ends_with_its_exit() {
        let screen = screen(&[
            Event::ShellOutput("a\u{1b}[31mb\u{1b}[0m\tc\r\n"),
            Event::ShellOutput("partial"),
            Event::ShellFinished {
                code: Some(1),
                problem: None,
            },
        ]);

        let expected = format!(
            "  ab    c\n  partial\n  {} {}\n\n",
            "└".dim(),
            "exit 1".red()
        );
        assert_eq!(screen, expected);
    }

    #[test]
    fn a_cut_off_reply_says_so_below_its_text() {
        let screen = screen(&[Event::Text("Half"), Event::Truncated]);

        let expected = format!(
            "Half\n\n  {}\n\n",
            "The reply hit the output limit and was cut off".dim()
        );
        assert_eq!(screen, expected);
    }

    #[test]
    fn a_refusal_is_shown_as_an_error() {
        let screen = screen(&[Event::Refused]);

        assert!(
            screen.contains(
                &"The model declined to answer; rephrase, or try another model with /model"
                    .red()
                    .to_string()
            )
        );
    }

    #[test]
    fn a_replayed_prompt_looks_like_a_sent_one() {
        let screen = screen(&[Event::Prompt("fix it"), Event::Text("Done.\n")]);

        assert_eq!(screen, format!("{}fix it\n\nDone.\n\n", "› ".dim()));
    }

    #[test]
    fn a_replayed_shell_command_has_its_red_bang() {
        let screen = screen(&[
            Event::ShellCommand("ls"),
            Event::ShellOutput("a\n"),
            Event::ShellFinished {
                code: Some(0),
                problem: None,
            },
        ]);

        assert!(screen.starts_with(&format!("{}ls\n\n  a\n", "! ".red())));
    }

    #[test]
    fn an_interrupted_turn_says_so_in_red() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.begin_turn(Work::Turn);
        terminal.finish_turn(Ending::Interrupted);

        assert!(visible(&terminal.out).contains(&"Interrupted".red().to_string()));
    }

    #[test]
    fn a_blocked_shell_command_shows_why() {
        let screen = screen(&[Event::ShellFinished {
            code: None,
            problem: Some("blocked: `rm -rf ~` deletes home"),
        }]);

        let expected = format!(
            "  {} {}\n\n",
            "└".dim(),
            "blocked: `rm -rf ~` deletes home".red()
        );
        assert_eq!(screen, expected);
    }

    #[test]
    fn a_running_command_shows_running() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer(""));
        terminal.begin_turn(Work::Shell);

        let expected = format!("⠋ Running {}\n› ", "(0s)".dim());
        assert_eq!(visible(&terminal.out), expected);
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(elapsed(Duration::from_secs(9)), "9s");
        assert_eq!(elapsed(Duration::from_secs(65)), "1m 05s");
    }
}

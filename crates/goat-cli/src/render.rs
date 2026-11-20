use std::io::{self, BufWriter, Stdout, Write};
use std::mem;
use std::time::{Duration, Instant};

use crossterm::style::{Color, Stylize};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, queue};
use goat_core::session::{Event, Sink};
use goat_core::tool::{self, Kind};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::input::fit;
use crate::markdown::{Markdown, Style};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INDENT: &str = "  ";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const GOAT: [&str; 3] = ["▀▀▄▄      ▄▄▀▀", "  ▄▄▀████▀▄▄  ", "     ▀██▀     "];
const GAP: &str = "   ";
const PRIMARY: Color = Color::AnsiValue(141);

pub(crate) struct Terminal<W: Write = BufWriter<Stdout>> {
    out: W,
    columns: fn() -> usize,
    pending: String,
    composer: Option<Frame>,
    live: Option<usize>,
    is_updating: bool,
    has_output: bool,
    section: Option<Section>,
    markdown: Markdown,
    calls: usize,
    activity: Option<Activity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Reply,
    Note,
    Reasoning,
    Text,
    Run,
}

struct Activity {
    started: Instant,
    frame: usize,
}

pub(crate) struct Frame {
    pub(crate) rows: Vec<String>,
    pub(crate) cursor: Spot,
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
            live: None,
            is_updating: false,
            has_output: false,
            section: None,
            markdown: Markdown::default(),
            calls: 0,
            activity: None,
        }
    }

    pub(crate) fn columns(&self) -> usize {
        (self.columns)()
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

    pub(crate) fn begin_turn(&mut self) {
        self.erase();
        self.activity = Some(Activity {
            started: Instant::now(),
            frame: 0,
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
            Ending::Interrupted => self.note("Interrupted"),
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
                Style::Fence | Style::Marker => self.put(&format!("{}", text.dim())),
            }
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
        self.enter(Section::Run);
        let branch = if self.calls == 0 { "└" } else { " " };
        self.calls += 1;
        let verb = match call.kind {
            Kind::Bash => "Bash",
            Kind::Read => "Read",
            Kind::Edit => "Edit",
            Kind::Write => "Write",
        };
        let mut lines = call.target.lines();
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
            format!("{} Working {}", SPINNER[activity.frame], took.dim())
        });
        let above = wrapped.len() + usize::from(spinner.is_some());
        let composer = self.composer.iter().flat_map(|frame| &frame.rows);
        let shown = wrapped
            .iter()
            .chain(&spinner)
            .chain(composer)
            .map(String::as_str);
        let mut count = 0;
        for (index, row) in shown.enumerate() {
            if index > 0 {
                write!(self.out, "\r\n").ok();
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
        match event {
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
            Event::CallFailed { problem } => {
                self.enter(Section::Run);
                self.put(&format!("{INDENT}{INDENT}{}\n", first_line(problem).red()));
            }
            Event::RunFinished { outcome } if outcome.is_error => {
                self.enter(Section::Run);
                self.put(&format!("{INDENT}{}\n", first_line(&outcome.output).red()));
            }
            Event::RunFinished { .. } | Event::Usage(_) => {}
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
        }
    }

    fn call(kind: Kind, target: &str) -> Call {
        Call {
            kind,
            target: target.into(),
        }
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

        let expected = format!(
            "{}   {} {}\n{}   {}\n{}   {}\n\n",
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
        terminal.begin_turn();
        terminal.emit(Event::Text("Hello\nwor"));

        let expected = format!("Hello\nwor\n⠋ Working {}\n› next", "(0s)".dim());
        assert_eq!(visible(&terminal.out), expected);
    }

    #[test]
    fn a_finished_turn_leaves_only_the_prompt_below() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer(""));
        terminal.begin_turn();
        terminal.emit(Event::Text("Done."));
        terminal.finish_turn(Ending::Complete);
        terminal.settle();

        assert_eq!(visible(&terminal.out), "Done.\n\n› ");
    }

    #[test]
    fn the_prompt_keeps_its_row_when_a_turn_ends() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.show(composer(""));
        terminal.begin_turn();
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
    fn durations_are_short() {
        assert_eq!(elapsed(Duration::from_secs(9)), "9s");
        assert_eq!(elapsed(Duration::from_secs(65)), "1m 05s");
    }
}

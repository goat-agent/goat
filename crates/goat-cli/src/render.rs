use std::io::{self, Stdout, Write};
use std::mem;
use std::time::{Duration, Instant};

use crossterm::queue;
use crossterm::style::Stylize;
use crossterm::terminal::{self, ClearType};
use goat_core::session::{Event, Sink};
use goat_core::tool::{self, Kind};

use crate::input::fit;
use crate::markdown::{Markdown, Style};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INDENT: &str = "  ";

pub(crate) struct Terminal<W: Write = Stdout> {
    out: W,
    columns: fn() -> usize,
    at_line_start: bool,
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
    is_shown: bool,
}

pub(crate) enum Ending {
    Complete,
    Interrupted,
    Failed(String),
}

impl Terminal {
    pub(crate) fn new() -> Self {
        Self::with_writer(io::stdout(), terminal_columns)
    }
}

impl<W: Write> Terminal<W> {
    fn with_writer(out: W, columns: fn() -> usize) -> Self {
        Self {
            out,
            columns,
            at_line_start: true,
            has_output: false,
            section: None,
            markdown: Markdown::default(),
            calls: 0,
            activity: None,
        }
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
        self.activity = Some(Activity {
            started: Instant::now(),
            frame: 0,
            is_shown: false,
        });
    }

    pub(crate) fn tick(&mut self) {
        if !self.at_line_start {
            return;
        }
        let Some(activity) = &mut self.activity else {
            return;
        };
        activity.frame = (activity.frame + 1) % SPINNER.len();
        activity.is_shown = true;
        let spinner = SPINNER[activity.frame];
        let took = format!("({})", elapsed(activity.started.elapsed()));
        queue!(self.out, terminal::Clear(ClearType::CurrentLine)).ok();
        write!(self.out, "\r{spinner} Working {}", took.dim()).ok();
        self.flush();
    }

    pub(crate) fn finish_turn(&mut self, ending: Ending) {
        self.close_section();
        self.hide_activity();
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
        if !self.at_line_start {
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
            if !self.at_line_start {
                self.put("\n");
            }
            return;
        }
        if self.at_line_start {
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
        let shown = fit(&target, room);
        let cut = if shown.len() < target.len() {
            "…"
        } else {
            ""
        };
        self.put(&format!(
            "{INDENT}{} {verb} {}\n",
            branch.dim(),
            format!("{shown}{cut}").dim()
        ));
    }

    fn put(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.hide_activity();
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            write!(self.out, "{first}").ok();
        }
        for line in lines {
            write!(self.out, "\r\n{line}").ok();
        }
        self.at_line_start = text.ends_with('\n');
        self.has_output = true;
    }

    fn hide_activity(&mut self) {
        let Some(activity) = &mut self.activity else {
            return;
        };
        if mem::take(&mut activity.is_shown) {
            queue!(self.out, terminal::Clear(ClearType::CurrentLine)).ok();
            write!(self.out, "\r").ok();
        }
    }

    fn flush(&mut self) {
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
        String::from_utf8(terminal.out).unwrap()
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
            "  {}{}\r\n  {}{}\r\n\r\nDone.\r\n\r\n",
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
            "{} Checking the build\r\n  {} Read {}\r\n  {} Bash {}\r\n\r\n",
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
            "  {} Bash {}\r\n\r\n",
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

        let expected = format!("  {}\r\n\r\n", "SyntaxError: unexpected token".red());
        assert_eq!(screen, expected);
    }

    #[test]
    fn markdown_text_is_styled_while_streaming() {
        let screen = screen(&[Event::Text("Run `cargo"), Event::Text(" test`\n")]);

        let expected = format!("Run {}{}\r\n\r\n", "cargo".cyan(), " test".cyan());
        assert_eq!(screen, expected);
    }

    #[test]
    fn output_after_a_prompt_starts_after_a_blank_line() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.settle();
        terminal.note("Switched to gpt-6.2-sol");

        let expected = format!("\r\n  {}\r\n", "Switched to gpt-6.2-sol".dim());
        assert_eq!(String::from_utf8(terminal.out).unwrap(), expected);
    }

    #[test]
    fn a_command_result_hangs_under_the_command() {
        let mut terminal = Terminal::with_writer(Vec::new(), || 40);
        terminal.settle();
        terminal.outcome(Ok("Set effort to high"));
        terminal.outcome(Err("no account offers gpt-6.3\nretry later"));

        let expected = format!(
            "  {} Set effort to high\r\n  {} {}\r\n  {} {}\r\n",
            "└".dim(),
            "└".dim(),
            "no account offers gpt-6.3".red(),
            " ".dim(),
            "retry later".red()
        );
        assert_eq!(String::from_utf8(terminal.out).unwrap(), expected);
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(elapsed(Duration::from_secs(9)), "9s");
        assert_eq!(elapsed(Duration::from_secs(65)), "1m 05s");
    }
}

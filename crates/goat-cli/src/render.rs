use std::io::{self, Stdout, Write};

use crossterm::style::Stylize;
use goat_core::provider::Usage;
use goat_core::session::{Event, Sink};

pub(crate) struct Terminal<W: Write = Stdout> {
    out: W,
    at_line_start: bool,
    reasoning: Option<Reasoning>,
    usage: Option<Usage>,
}

enum Reasoning {
    OnLine,
    BreakPending,
}

impl Terminal {
    pub(crate) fn new() -> Self {
        Self::with_writer(io::stdout())
    }
}

impl<W: Write> Terminal<W> {
    fn with_writer(out: W) -> Self {
        Self {
            out,
            at_line_start: true,
            reasoning: None,
            usage: None,
        }
    }

    pub(crate) fn note(&mut self, text: &str) {
        self.line(&format!("{}", text.dim()));
    }

    pub(crate) fn error(&mut self, text: &str) {
        self.line(&format!("{}", text.red()));
    }

    pub(crate) fn finish_turn(&mut self) {
        self.end_line();
        if let Some(usage) = self.usage.take() {
            let summary = format!(
                "  {} in ({} cached) · {} out",
                tokens(usage.input),
                tokens(usage.cached),
                tokens(usage.output)
            );
            self.note(&summary);
        }
        self.flush();
    }

    fn line(&mut self, text: &str) {
        self.end_line();
        writeln!(self.out, "{text}").ok();
        self.at_line_start = true;
        self.flush();
    }

    fn reasoning(&mut self, chunk: &str) {
        for (index, piece) in chunk.split('\n').enumerate() {
            if index > 0 && self.reasoning.is_some() {
                self.reasoning = Some(Reasoning::BreakPending);
            }
            let piece = piece.replace("**", "");
            if piece.is_empty() {
                continue;
            }
            match self.reasoning {
                None => {
                    self.end_line();
                    write!(self.out, "  ").ok();
                }
                Some(Reasoning::BreakPending) => {
                    write!(self.out, "\n  ").ok();
                }
                Some(Reasoning::OnLine) => {}
            }
            write!(self.out, "{}", piece.dim()).ok();
            self.reasoning = Some(Reasoning::OnLine);
            self.at_line_start = false;
        }
        self.flush();
    }

    fn end_line(&mut self) {
        self.reasoning = None;
        if !self.at_line_start {
            writeln!(self.out).ok();
            self.at_line_start = true;
        }
    }

    fn flush(&mut self) {
        self.out.flush().ok();
    }
}

impl<W: Write> Sink for Terminal<W> {
    fn emit(&mut self, event: Event<'_>) {
        match event {
            Event::Text(text) => {
                if text.is_empty() {
                    return;
                }
                if self.reasoning.is_some() {
                    self.end_line();
                }
                write!(self.out, "{text}").ok();
                self.at_line_start = text.ends_with('\n');
                self.flush();
            }
            Event::RunStarted { description } => self.note(&format!("  {description}")),
            Event::Call { summary } => self.note(&format!("    {summary}")),
            Event::CallFailed { problem } => self.error(&format!("    {}", first_line(problem))),
            Event::RunFinished { outcome } if outcome.is_error => {
                self.error(&format!("    {}", first_line(&outcome.output)));
            }
            Event::RunFinished { .. } => {}
            Event::Reasoning(chunk) => self.reasoning(chunk),
            Event::Usage(usage) => self.usage = Some(usage),
            Event::Compacting => self.note("  compacting the conversation"),
        }
    }
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
}

fn tokens(count: u64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    let tenths = (count + 50) / 100;
    format!("{}.{}k", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(events: &[Event<'_>]) -> String {
        let mut terminal = Terminal::with_writer(Vec::new());
        for event in events {
            terminal.emit(*event);
        }
        String::from_utf8(terminal.out).unwrap()
    }

    #[test]
    fn reasoning_is_dimmed_and_text_starts_on_the_next_line() {
        let screen = screen(&[
            Event::Reasoning("**Checking"),
            Event::Reasoning(" tests**"),
            Event::Text("Done."),
        ]);

        let expected = format!("  {}{}\nDone.", "Checking".dim(), " tests".dim());
        assert_eq!(screen, expected);
    }

    #[test]
    fn reasoning_breaks_collapse_into_one_indented_line_break() {
        let screen = screen(&[
            Event::Reasoning("\n"),
            Event::Reasoning("First"),
            Event::Reasoning("\n\n"),
            Event::Reasoning("Second"),
        ]);

        let expected = format!("  {}\n  {}", "First".dim(), "Second".dim());
        assert_eq!(screen, expected);
    }

    #[test]
    fn token_counts_are_compact() {
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1234), "1.2k");
        assert_eq!(tokens(200_000), "200.0k");
    }
}

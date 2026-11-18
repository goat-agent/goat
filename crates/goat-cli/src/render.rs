use std::io::{self, Stdout, Write};

use crossterm::style::Stylize;
use goat_core::provider::Usage;
use goat_core::session::{Event, Sink};

pub(crate) struct Terminal {
    out: Stdout,
    at_line_start: bool,
    usage: Option<Usage>,
}

impl Terminal {
    pub(crate) fn new() -> Self {
        Self {
            out: io::stdout(),
            at_line_start: true,
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

    fn end_line(&mut self) {
        if !self.at_line_start {
            writeln!(self.out).ok();
            self.at_line_start = true;
        }
    }

    fn flush(&mut self) {
        self.out.flush().ok();
    }
}

impl Sink for Terminal {
    fn emit(&mut self, event: Event<'_>) {
        match event {
            Event::Text(text) => {
                if text.is_empty() {
                    return;
                }
                write!(self.out, "{text}").ok();
                self.at_line_start = text.ends_with('\n');
                self.flush();
            }
            Event::ToolStarted { summary, .. } => self.note(&format!("  {summary}")),
            Event::ToolFinished { outcome } if outcome.is_error => {
                let first = outcome
                    .output
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default();
                self.error(&format!("    {first}"));
            }
            Event::ToolFinished { .. } => {}
            Event::Usage(usage) => self.usage = Some(usage),
            Event::Compacting => self.note("  compacting the conversation"),
        }
    }
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

    #[test]
    fn token_counts_are_compact() {
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1234), "1.2k");
        assert_eq!(tokens(200_000), "200.0k");
    }
}

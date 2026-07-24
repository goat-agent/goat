use std::io::IsTerminal;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy)]
pub enum ColorMode {
    Plain,
    Ansi,
}

impl ColorMode {
    pub fn detect() -> Self {
        Self::detect_stream(std::io::stdout().is_terminal())
    }

    pub fn detect_stderr() -> Self {
        Self::detect_stream(std::io::stderr().is_terminal())
    }

    fn detect_stream(is_terminal: bool) -> Self {
        if std::env::var_os("NO_COLOR").is_some() || !is_terminal {
            Self::Plain
        } else {
            Self::Ansi
        }
    }

    pub fn paint(self, text: impl AsRef<str>, palette: Palette) -> String {
        let text = text.as_ref();
        match (self, palette.code()) {
            (Self::Ansi, Some(code)) => format!("{code}{text}\x1b[0m"),
            _ => text.to_owned(),
        }
    }

    pub fn cell(self, text: impl AsRef<str>, palette: Palette, width: usize) -> String {
        let shown = truncate_to_width(text.as_ref(), width);
        let pad = width.saturating_sub(shown.width());
        format!("{}{}", self.paint(&shown, palette), " ".repeat(pad))
    }
}

#[derive(Clone, Copy)]
pub enum Palette {
    Plain,
    Info,
    Local,
    Muted,
    Provider,
    Success,
    Value,
    Warning,
}

impl Palette {
    fn code(self) -> Option<&'static str> {
        Some(match self {
            Self::Plain => return None,
            Self::Info => "\x1b[38;5;75m",
            Self::Local => "\x1b[38;5;214m",
            Self::Muted => "\x1b[38;5;245m",
            Self::Provider => "\x1b[1;38;5;219m",
            Self::Success => "\x1b[38;5;84m",
            Self::Value => "\x1b[38;5;252m",
            Self::Warning => "\x1b[38;5;209m",
        })
    }
}

pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for c in s.chars() {
        let char_width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + char_width + 1 > max_width {
            break;
        }
        out.push(c);
        width += char_width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::{ColorMode, Palette, truncate_to_width};

    #[test]
    fn cell_respects_display_width() {
        let color = ColorMode::Plain;
        let cell = color.cell("한글provider", Palette::Provider, 12);
        assert_eq!(cell.width(), 12);
    }

    #[test]
    fn truncate_fits_width() {
        let truncated = truncate_to_width("very long provider name", 10);
        assert!(truncated.width() <= 10);
        assert!(truncated.ends_with('…'));
    }
}

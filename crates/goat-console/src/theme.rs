use std::fmt;

use dialoguer::theme::Theme;

use crate::color::{ColorMode, Palette};

const QUESTION: &str = "?";
const CURSOR: &str = "❯";

pub struct GoatTheme;

pub fn goat_theme() -> &'static GoatTheme {
    static THEME: GoatTheme = GoatTheme;
    &THEME
}

impl GoatTheme {
    fn question() -> String {
        ColorMode::detect_stderr().paint(QUESTION, Palette::Provider)
    }

    fn cursor() -> String {
        ColorMode::detect_stderr().paint(CURSOR, Palette::Provider)
    }
}

impl Theme for GoatTheme {
    fn format_prompt(&self, f: &mut dyn fmt::Write, prompt: &str) -> fmt::Result {
        write!(f, "{} {prompt}", Self::question())
    }

    fn format_error(&self, f: &mut dyn fmt::Write, err: &str) -> fmt::Result {
        write!(
            f,
            "{}",
            ColorMode::detect_stderr().paint(err, Palette::Warning)
        )
    }

    fn format_select_prompt(&self, f: &mut dyn fmt::Write, prompt: &str) -> fmt::Result {
        self.format_prompt(f, prompt)
    }

    fn format_input_prompt(
        &self,
        f: &mut dyn fmt::Write,
        prompt: &str,
        default: Option<&str>,
    ) -> fmt::Result {
        match default {
            Some(default) => {
                let hint = ColorMode::detect_stderr().paint(format!("({default})"), Palette::Muted);
                write!(f, "{} {prompt} {hint}", Self::question())
            }
            None => self.format_prompt(f, prompt),
        }
    }

    fn format_select_prompt_item(
        &self,
        f: &mut dyn fmt::Write,
        text: &str,
        active: bool,
    ) -> fmt::Result {
        if active {
            write!(f, "{} {text}", Self::cursor())
        } else {
            write!(f, "  {text}")
        }
    }

    fn format_password_prompt(&self, f: &mut dyn fmt::Write, prompt: &str) -> fmt::Result {
        self.format_prompt(f, prompt)
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::GoatTheme;
    use dialoguer::theme::Theme;

    fn format_item(active: bool, text: &str) -> String {
        let mut buf = String::new();
        GoatTheme
            .format_select_prompt_item(&mut buf, text, active)
            .unwrap();
        buf
    }

    #[test]
    fn select_cursor_keeps_row_width() {
        let plain = "openrouter      missing";
        assert_eq!(
            format_item(true, plain).width(),
            format_item(false, plain).width()
        );
    }

    #[test]
    fn select_cursor_keeps_ansi_row_width() {
        let colored = "\x1b[1;38;5;219mopenrouter\x1b[0m      \x1b[38;5;209mmissing\x1b[0m";
        assert_eq!(
            format_item(true, colored).width(),
            format_item(false, colored).width()
        );
    }

    #[test]
    fn select_cursor_uses_fixed_two_column_prefix() {
        assert!(format_item(true, "x").starts_with("❯ "));
        assert!(format_item(false, "x").starts_with("  "));
    }
}

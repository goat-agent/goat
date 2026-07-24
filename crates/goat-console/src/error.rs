use crate::color::{ColorMode, Palette};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ConsoleError(pub String);

pub type ConsoleResult<T> = Result<T, ConsoleError>;

pub fn report(message: impl Into<String>) -> ConsoleError {
    ConsoleError(format_failure(&message.into(), None))
}

pub fn report_hint(message: impl Into<String>, hint: impl Into<String>) -> ConsoleError {
    ConsoleError(format_failure(&message.into(), Some(hint.into())))
}

pub fn fail(message: impl Into<String>) -> ConsoleResult<()> {
    Err(report(message))
}

pub fn fail_hint(message: impl Into<String>, hint: impl Into<String>) -> ConsoleResult<()> {
    Err(report_hint(message, hint))
}

pub(crate) fn dialoguer_error(err: impl std::fmt::Display) -> ConsoleError {
    report(format!("prompt failed: {err}"))
}

pub fn format_failure(message: &str, hint: Option<String>) -> String {
    let color = ColorMode::detect_stderr();
    let mut lines = vec![format!(
        "{} {}",
        color.paint("error:", Palette::Warning),
        color.paint(message, Palette::Value)
    )];
    if let Some(hint) = hint {
        lines.push(format!(
            "{} {}",
            color.paint("hint:", Palette::Muted),
            color.paint(hint, Palette::Muted)
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::format_failure;

    #[test]
    fn failure_format_includes_hint() {
        let text = format_failure(
            "unknown provider",
            Some("run goat provider list".to_owned()),
        );
        assert!(text.contains("error:"));
        assert!(text.contains("unknown provider"));
        assert!(text.contains("hint:"));
        assert!(text.contains("run goat provider list"));
    }
}

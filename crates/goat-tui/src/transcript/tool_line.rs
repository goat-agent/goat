use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::gutter::hang;
use super::tool_gist::{ToolLineCtx, transcript_sig};
use crate::{symbols, theme::Theme};

pub(crate) struct ToolRowInput<'a> {
    pub name: &'a str,
    pub display_primary: &'a str,
    pub marker: &'a str,
    pub marker_style: Style,
    pub theme: Theme,
    pub width: u16,
    pub line_ctx: ToolLineCtx<'a>,
}

pub(crate) fn tool_row(input: &ToolRowInput<'_>) -> Vec<Line<'static>> {
    let sig = transcript_sig(input.name, input.display_primary, &input.line_ctx);
    let gutter = Span::styled(format!("{} ", input.marker), input.marker_style);
    hang(
        &[Line::from(signature_spans(&sig, input.theme))],
        gutter,
        input.width,
    )
}

pub(crate) fn tool_marker(status: &super::item::ToolStatus, theme: Theme) -> (&'static str, Style) {
    use super::item::ToolStatus;
    use goat_protocol::ToolOutcome;
    match status {
        ToolStatus::Running => (symbols::SPINNER[0], theme.accent()),
        ToolStatus::Done(ToolOutcome { ok: true, .. }) => (symbols::ui::CHECK, theme.success()),
        ToolStatus::Done(ToolOutcome { ok: false, .. }) => (symbols::ui::CROSS, theme.error()),
    }
}

fn signature_spans(sig: &str, theme: Theme) -> Vec<Span<'static>> {
    let Some(open) = sig.find('(') else {
        return vec![Span::styled(sig.to_owned(), theme.tool_fn())];
    };
    let name = sig[..open].to_owned();
    let tail = &sig[open..];
    if !tail.ends_with(')') || tail.len() < 2 {
        return vec![
            Span::styled(name, theme.tool_fn()),
            Span::styled(tail.to_owned(), theme.muted()),
        ];
    }
    let inner = &tail[1..tail.len() - 1];
    let mut spans = vec![
        Span::styled(name, theme.tool_fn()),
        Span::styled("(".to_owned(), theme.muted()),
    ];
    if inner.is_empty() {
        spans.push(Span::styled(")".to_owned(), theme.muted()));
        return spans;
    }
    for (i, part) in inner.split(", ").enumerate() {
        if i > 0 {
            spans.push(Span::styled(", ".to_owned(), theme.muted()));
        }
        spans.push(Span::styled(part.to_owned(), theme.tool_arg_value()));
    }
    spans.push(Span::styled(")".to_owned(), theme.muted()));
    spans
}

use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    layout::{OVERLAY_CHROME_PLAIN, OVERLAY_W},
    overlay::{centered_rect, clamp_u16, hint_line, overlay_frame, overlay_layout_plain},
    symbols,
    theme::Theme,
};

const BINDINGS: [(&str, &str); 15] = [
    ("⇧↵ ⌥↵", "newline"),
    ("↑↓", "history · move cursor"),
    ("pgup pgdn", "scroll transcript by page"),
    ("home end", "transcript top · bottom"),
    ("⌃a ⌃e", "line start · end"),
    ("⌃w", "delete word"),
    ("⌃t", "expand · collapse thinking"),
    ("⌥← ⌥→", "word left · right"),
    ("⇥", "complete command"),
    ("esc", "interrupt · clear input ×2"),
    ("⌃c", "quit ×2"),
    ("/", "commands"),
    ("!", "shell command (first char)"),
    ("drag ⌘c", "select · copy transcript"),
    ("click", "open link"),
];

fn desired_height() -> u16 {
    clamp_u16(BINDINGS.len()).saturating_add(OVERLAY_CHROME_PLAIN)
}

pub fn render(frame: &mut Frame, area: Rect, theme: Theme) {
    let rect = centered_rect(area, OVERLAY_W, desired_height());
    let Some(inner) = overlay_frame(frame, rect, theme) else {
        return;
    };
    let (body_area, hint_area) = overlay_layout_plain(inner);

    let lines: Vec<Line> = BINDINGS
        .iter()
        .map(|(keys, action)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<10}"), theme.hint_key()),
                Span::styled((*action).to_owned(), theme.muted()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), body_area);

    frame.render_widget(
        Paragraph::new(hint_line(&[(symbols::key::ESC, "close")], theme)),
        hint_area,
    );
}

use goat_protocol::{AccountEntry, AuthMethod};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    overlay::{clamp_u16, hint_line, overlay_layout, selection_row},
    symbols,
    theme::Theme,
};

use super::state::{FIELD_LABEL_W, Field};

pub(super) fn appearance_row(
    theme: Theme,
    width: usize,
    selected: bool,
    label: &str,
    first_active: bool,
    first: &str,
    second: &str,
) -> Line<'static> {
    let label_style = if selected { theme.key() } else { theme.base() };
    let first_dot = if first_active {
        symbols::ui::DOT_FULL
    } else {
        symbols::ui::DOT_EMPTY
    };
    let second_dot = if first_active {
        symbols::ui::DOT_EMPTY
    } else {
        symbols::ui::DOT_FULL
    };
    let first_style = if first_active {
        theme.accent()
    } else {
        theme.muted()
    };
    let second_style = if first_active {
        theme.muted()
    } else {
        theme.accent()
    };
    selection_row(
        theme,
        selected,
        width,
        vec![
            Span::styled(format!("{label:<12}"), label_style),
            Span::styled(format!("{first_dot} {first:<6}"), first_style),
            Span::styled(format!("{second_dot} {second}"), second_style),
        ],
        None,
    )
}

pub(super) fn method_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::ApiKey => "api key",
        AuthMethod::OAuth => "browser",
        AuthMethod::ApiKeyOrOAuth => "api key / browser",
        AuthMethod::None => "no auth",
    }
}

pub(super) fn provider_method(entry: &AccountEntry) -> AuthMethod {
    entry.login
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_adding(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    method: AuthMethod,
    name: &str,
    key: &str,
    field: Field,
    error: Option<&str>,
) {
    let (title_area, body_area, hint_area) = overlay_layout(area);
    let title = Line::from(vec![
        Span::styled(format!(" {provider}"), theme.key()),
        Span::styled(
            format!("{}connect account", symbols::ui::SEPARATOR),
            theme.muted(),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    let api_key = !matches!(method, AuthMethod::OAuth);
    let value_cols = usize::from(body_area.width).saturating_sub(FIELD_LABEL_W + 1);
    let mut body_lines: Vec<Line> = Vec::new();

    let name_active = field == Field::Name;
    let name_label_style = if name_active {
        theme.accent()
    } else {
        theme.muted()
    };
    let shown_name = input_tail(name, value_cols);
    let name_cols = shown_name.width();
    body_lines.push(Line::from(vec![
        Span::styled(format!(" {:<FIELD_LABEL_W$}", "name"), name_label_style),
        Span::styled(shown_name, theme.base()),
    ]));

    if api_key {
        let key_active = field == Field::Key;
        let key_label_style = if key_active {
            theme.accent()
        } else {
            theme.muted()
        };
        let mask_cols = key.chars().count().min(value_cols);
        let masked = symbols::ui::MASK.repeat(mask_cols);
        body_lines.push(Line::from(vec![
            Span::styled(format!(" {:<FIELD_LABEL_W$}", "key"), key_label_style),
            Span::styled(masked, theme.base()),
        ]));
        if key_active {
            place_cursor(frame, body_area, 1, mask_cols);
        }
    }
    if name_active {
        place_cursor(frame, body_area, 0, name_cols);
    }

    if let Some(message) = error {
        body_lines.push(Line::from(Span::styled(
            format!(" {} {message}", symbols::ui::CROSS),
            theme.error(),
        )));
    }

    frame.render_widget(
        Paragraph::new(body_lines).wrap(Wrap { trim: false }),
        body_area,
    );

    let hints = if api_key {
        &[
            (symbols::key::ENTER, "save"),
            (symbols::key::TAB, "next field"),
            (symbols::key::ESC, "cancel"),
        ][..]
    } else {
        &[
            (symbols::key::ENTER, "device login"),
            (symbols::key::ESC, "cancel"),
        ][..]
    };
    frame.render_widget(Paragraph::new(hint_line(hints, theme)), hint_area);
}

fn input_tail(value: &str, max_cols: usize) -> String {
    if value.width() <= max_cols {
        return value.to_owned();
    }
    let mut cols = 0usize;
    let mut tail: Vec<char> = Vec::new();
    for c in value.chars().rev() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if cols + w > max_cols {
            break;
        }
        cols += w;
        tail.push(c);
    }
    tail.iter().rev().collect()
}

fn place_cursor(frame: &mut Frame, area: Rect, row: u16, value_cols: usize) {
    let prefix = clamp_u16(1 + FIELD_LABEL_W);
    let col = clamp_u16(value_cols).saturating_add(prefix);
    let x = area.x + col.min(area.width.saturating_sub(1));
    let y = area.y + row.min(area.height.saturating_sub(1));
    frame.set_cursor_position((x, y));
}

pub(super) fn render_waiting(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    name: &str,
    status: Option<&str>,
) {
    let (title_area, body_area, _) = overlay_layout(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {provider}"), theme.key()),
            Span::styled(symbols::ui::SEPARATOR, theme.muted()),
            Span::styled(name.to_owned(), theme.base()),
        ])),
        title_area,
    );
    if let Some(msg) = status {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {msg}"), theme.muted()))),
            body_area,
        );
    }
}

pub(super) fn render_choosing(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    method: AuthMethod,
) {
    let (title_area, list_area, hint_area) = overlay_layout(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {provider}"), theme.key()),
            Span::styled(
                format!("{}sign in with", symbols::ui::SEPARATOR),
                theme.muted(),
            ),
        ])),
        title_area,
    );

    let width = usize::from(list_area.width);
    let options = [
        ("api key", AuthMethod::ApiKey),
        ("browser", AuthMethod::OAuth),
    ];
    let lines: Vec<Line> = options
        .iter()
        .map(|(label, auth)| {
            let selected = *auth == method;
            let style = if selected { theme.key() } else { theme.base() };
            selection_row(
                theme,
                selected,
                width,
                vec![Span::styled((*label).to_owned(), style)],
                None,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);
    frame.render_widget(
        Paragraph::new(hint_line(
            &[
                (symbols::key::ARROWS_UPDOWN, "choose"),
                (symbols::key::ENTER, "continue"),
                (symbols::key::ESC, "cancel"),
            ],
            theme,
        )),
        hint_area,
    );
}

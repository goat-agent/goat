use goat_protocol::Effort;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    layout::{LIST_MAX, OVERLAY_CHROME, OVERLAY_W},
    overlay::{centered_rect, clamp_u16, hint_line, overlay_frame, overlay_layout, selection_row},
    symbols,
    theme::Theme,
};

pub enum EffortOutcome {
    NoOp,
    Selected(Effort),
}

pub struct EffortPicker {
    label: String,
    options: Vec<Effort>,
    cursor: usize,
    scroll: usize,
    empty_message: Option<String>,
}

impl EffortPicker {
    pub fn new(label: String, options: Vec<Effort>, current: Option<Effort>) -> Self {
        let empty_message = if options.is_empty() {
            Some("This model does not support reasoning effort.".to_owned())
        } else {
            None
        };
        let cursor = current
            .and_then(|cur| options.iter().position(|opt| *opt == cur))
            .unwrap_or(0);
        Self {
            label,
            options,
            cursor,
            scroll: 0,
            empty_message,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.options.is_empty()
    }

    fn cap(&self) -> usize {
        self.options.len().min(LIST_MAX)
    }

    pub fn move_up(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 >= self.options.len() {
            return;
        }
        self.cursor += 1;
        let cap = self.cap();
        if self.cursor >= self.scroll + cap {
            self.scroll = self.cursor + 1 - cap;
        }
    }

    pub fn choose(&self) -> EffortOutcome {
        self.options
            .get(self.cursor)
            .map_or(EffortOutcome::NoOp, |effort| {
                EffortOutcome::Selected(*effort)
            })
    }

    pub fn desired_height(&self) -> u16 {
        clamp_u16(self.cap().max(1)).saturating_add(OVERLAY_CHROME)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        let (context_area, list_area, hint_area) = overlay_layout(inner);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", self.label),
                theme.muted(),
            ))),
            context_area,
        );

        if let Some(msg) = &self.empty_message {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(format!(" {msg}"), theme.muted()))),
                list_area,
            );
            frame.render_widget(
                Paragraph::new(hint_line(&[(symbols::key::ESC, "close")], theme)),
                hint_area,
            );
            return;
        }

        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height).max(1);
        let scroll = self.scroll.min(self.cursor);

        let lines: Vec<Line> = self
            .options
            .iter()
            .enumerate()
            .skip(scroll)
            .take(rows)
            .map(|(index, effort)| {
                let selected = index == self.cursor;
                let name_style = if selected { theme.key() } else { theme.base() };
                selection_row(
                    theme,
                    selected,
                    width,
                    vec![Span::styled(effort.as_str().to_owned(), name_style)],
                    None,
                )
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "navigate"),
                    (symbols::key::ENTER, "select"),
                    (symbols::key::ESC, "close"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

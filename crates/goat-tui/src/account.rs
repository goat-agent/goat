use goat_protocol::{AccountChoice, ModelTarget};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{layout::LIST_MAX, overlay::selection_row, theme::Theme};

pub struct AccountMenu {
    choices: Vec<AccountChoice>,
    cursor: usize,
}

impl AccountMenu {
    pub fn new(choices: Vec<AccountChoice>) -> Self {
        Self { choices, cursor: 0 }
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.choices.len() {
            self.cursor += 1;
        }
    }

    pub fn selected(&self) -> Option<ModelTarget> {
        self.choices
            .get(self.cursor)
            .map(|choice| choice.target.clone())
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.choices.len().clamp(1, LIST_MAX);
        u16::try_from(rows).unwrap_or(u16::MAX)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let width = usize::from(area.width);
        let visible = usize::from(area.height).max(1);
        let start = if self.cursor >= visible {
            self.cursor + 1 - visible
        } else {
            0
        };
        let lines: Vec<Line> = self
            .choices
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(pos, choice)| {
                let selected = pos == self.cursor;
                let name_style = if selected { theme.key() } else { theme.base() };
                selection_row(
                    theme,
                    selected,
                    width,
                    vec![Span::styled(choice.display.clone(), name_style)],
                    None,
                )
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }
}

use goat_protocol::{AccountChoice, ModelTarget};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use goat_command::{Theme, layout::LIST_MAX, overlay::selection_row};

pub struct AccountScreen {
    choices: Vec<AccountChoice>,
    cursor: usize,
    done: bool,
}

impl AccountScreen {
    pub fn new(choices: Vec<AccountChoice>) -> Self {
        Self {
            choices,
            cursor: 0,
            done: false,
        }
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

impl goat_command::Screen for AccountScreen {
    fn placement(&self) -> goat_command::Placement {
        goat_command::Placement::Panel {
            height: self.desired_height(),
            hints: Some(vec![
                goat_command::KeyHint {
                    key: goat_command::symbols::key::ARROWS_UPDOWN,
                    label: "navigate",
                },
                goat_command::KeyHint {
                    key: goat_command::symbols::key::ENTER,
                    label: "select",
                },
                goat_command::KeyHint {
                    key: goat_command::symbols::key::ESC,
                    label: "cancel",
                },
            ]),
            composer_focused: true,
        }
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        AccountScreen::render(self, frame, area, *theme);
    }

    fn handle_input(
        &mut self,
        event: &crossterm::event::Event,
        _session: &mut dyn goat_command::Session,
    ) -> goat_command::InputOutcome {
        use crossterm::event::{Event as InputEvent, KeyCode};
        use goat_command::{CommandEffect, InputOutcome, ScreenOutcome};
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        if goat_command::keymap::ctrl_key(key).is_some() {
            return InputOutcome::Handled(if goat_command::keymap::ctrl_key(key) == Some('c') {
                ScreenOutcome::Close
            } else {
                ScreenOutcome::Continue
            });
        }
        let outcome = match key.code {
            KeyCode::Esc => ScreenOutcome::Close,
            KeyCode::Up => {
                self.move_up();
                ScreenOutcome::Continue
            }
            KeyCode::Down => {
                self.move_down();
                ScreenOutcome::Continue
            }
            KeyCode::Enter => {
                let Some(target) = self.selected() else {
                    return InputOutcome::Handled(ScreenOutcome::Continue);
                };
                self.done = true;
                ScreenOutcome::Effect(CommandEffect::Dispatch(vec![
                    goat_protocol::Op::SelectModel { target },
                ]))
            }
            _ => ScreenOutcome::Continue,
        };
        InputOutcome::Handled(outcome)
    }

    fn tick(&mut self) -> goat_command::ScreenOutcome {
        if self.done {
            goat_command::ScreenOutcome::Close
        } else {
            goat_command::ScreenOutcome::Continue
        }
    }
}

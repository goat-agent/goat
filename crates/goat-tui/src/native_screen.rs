use std::sync::Arc;

use crossterm::event::{Event as InputEvent, KeyCode};
use goat_command::{CommandEffect, InputOutcome, Placement, Screen, ScreenOutcome, Session, Theme};
use goat_protocol::{Event, Op, TaskId, ToolCallId, ToolImageData};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::ask::{AskOutcome, AskPicker};

pub struct AskScreen {
    picker: AskPicker,
    task: TaskId,
    call: ToolCallId,
    done: bool,
}

impl AskScreen {
    pub fn new(picker: AskPicker, task: TaskId, call: ToolCallId) -> Self {
        Self {
            picker,
            task,
            call,
            done: false,
        }
    }

    pub fn call(&self) -> ToolCallId {
        self.call
    }

    fn submit(&mut self, answers: Vec<String>) -> ScreenOutcome {
        self.done = true;
        ScreenOutcome::Effect(CommandEffect::Dispatch(vec![Op::Answer {
            id: self.task,
            call: self.call,
            answers,
        }]))
    }
}

impl Screen for AskScreen {
    fn placement(&self) -> Placement {
        Placement::Full {
            reserve_bottom: Some(self.picker.desired_height()),
        }
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.picker.render(frame, area, *theme);
    }

    fn handle_input(&mut self, event: &InputEvent, _session: &mut dyn Session) -> InputOutcome {
        if let InputEvent::Paste(text) = event {
            self.picker.insert_str(text);
            return InputOutcome::Handled(ScreenOutcome::Continue);
        }
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        if let Some(ch) = goat_command::keymap::ctrl_key(key) {
            let outcome = if ch == 'c' {
                self.done = true;
                ScreenOutcome::Effect(CommandEffect::Dispatch(vec![Op::Interrupt {
                    id: self.task,
                }]))
            } else {
                ScreenOutcome::Continue
            };
            return InputOutcome::Handled(outcome);
        }
        let outcome = match key.code {
            KeyCode::Esc => {
                if self.picker.is_confirming() || self.picker.is_typing() {
                    self.picker.go_back();
                    ScreenOutcome::Continue
                } else {
                    self.done = true;
                    ScreenOutcome::Effect(CommandEffect::Dispatch(vec![Op::Interrupt {
                        id: self.task,
                    }]))
                }
            }
            KeyCode::Up => {
                self.picker.move_up();
                ScreenOutcome::Continue
            }
            KeyCode::Down => {
                self.picker.move_down();
                ScreenOutcome::Continue
            }
            KeyCode::Left => {
                self.picker.go_back();
                ScreenOutcome::Continue
            }
            KeyCode::Right => match self.picker.skip() {
                AskOutcome::Submit(answers) => self.submit(answers),
                AskOutcome::Pending | AskOutcome::NoOp => ScreenOutcome::Continue,
            },
            KeyCode::Backspace => {
                self.picker.backspace();
                ScreenOutcome::Continue
            }
            KeyCode::Enter => match self.picker.choose() {
                AskOutcome::Submit(answers) => self.submit(answers),
                AskOutcome::Pending | AskOutcome::NoOp => ScreenOutcome::Continue,
            },
            KeyCode::Char(ch) => {
                if ch == ' ' && self.picker.wants_toggle() {
                    self.picker.toggle();
                } else {
                    self.picker.on_char(ch);
                }
                ScreenOutcome::Continue
            }
            _ => ScreenOutcome::Continue,
        };
        InputOutcome::Handled(outcome)
    }

    fn on_event(&mut self, event: &Event, _session: &mut dyn Session) -> ScreenOutcome {
        if matches!(event, Event::AskDismissed { call, .. } if *call == self.call) {
            ScreenOutcome::Close
        } else {
            ScreenOutcome::Continue
        }
    }

    fn tick(&mut self) -> ScreenOutcome {
        if self.done {
            ScreenOutcome::Close
        } else {
            ScreenOutcome::Continue
        }
    }
}

pub struct ImageZoomScreen {
    source: Box<ToolImageData>,
    picker: Option<Arc<ratatui_image::picker::Picker>>,
}

impl ImageZoomScreen {
    pub fn new(
        source: Box<ToolImageData>,
        picker: Option<Arc<ratatui_image::picker::Picker>>,
    ) -> Self {
        Self { source, picker }
    }
}

impl Screen for ImageZoomScreen {
    fn placement(&self) -> Placement {
        Placement::Full {
            reserve_bottom: None,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let [body, hint] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let image_area = body.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        if let Some(picker) = &self.picker {
            crate::screenshot::render_zoom(frame, image_area, picker, &self.source);
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " image preview unavailable in this terminal ",
                    theme.muted(),
                ))),
                image_area,
            );
        }
        frame.render_widget(
            Paragraph::new(crate::overlay::hint_line(
                &[(crate::symbols::key::ESC, "close")],
                *theme,
            )),
            hint,
        );
    }

    fn handle_input(&mut self, event: &InputEvent, _session: &mut dyn Session) -> InputOutcome {
        let outcome = match event {
            InputEvent::Key(_) | InputEvent::Mouse(_) => ScreenOutcome::Close,
            _ => return InputOutcome::Ignored,
        };
        InputOutcome::Handled(outcome)
    }
}

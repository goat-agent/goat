use crossterm::event::{Event as InputEvent, KeyCode};
use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, InputOutcome, ParameterSpec,
    ParameterValue, Placement, Screen, ScreenOutcome, Session, Theme, keymap,
    layout::{LIST_MAX, OVERLAY_CHROME, OVERLAY_W},
    overlay::{centered_rect, clamp_u16, hint_line, overlay_frame, overlay_layout, selection_row},
    symbols,
};
use goat_protocol::{Effort as EffortLevel, NotifyKind, Op};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

pub struct Effort;

impl Command for Effort {
    fn name(&self) -> &'static str {
        "effort"
    }

    fn description(&self) -> &'static str {
        "set reasoning effort"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "level".to_owned(),
            description: "off, low, medium, high, xhigh, or max".to_owned(),
            required: false,
            value: ParameterValue::Word,
        }])
    }

    fn run(&self, invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        let options = current_efforts(session);
        if let Some(level) = invocation.text("level") {
            let level = level.to_ascii_lowercase();
            let Some(effort) = EffortLevel::parse(&level) else {
                session.notify(NotifyKind::Error, format!("unknown effort: {level}"));
                return CommandEffect::Noop;
            };
            if !options.contains(&effort) {
                session.notify(
                    NotifyKind::Error,
                    format!("current model does not support effort: {level}"),
                );
                return CommandEffect::Noop;
            }
            let mut target = session.current_model().cloned().expect("model checked");
            target.effort = Some(effort);
            CommandEffect::Dispatch(vec![Op::SelectModel { target }])
        } else {
            let label = session.current_model().map_or_else(
                || "no model selected".to_owned(),
                |model| format!("{}/{}", model.provider, model.model),
            );
            let current = session.current_model().and_then(|model| model.effort);
            CommandEffect::Show(Box::new(EffortScreen::new(label, options, current)))
        }
    }
}

fn current_efforts(session: &dyn Session) -> Vec<EffortLevel> {
    let Some(model) = session.current_model() else {
        return Vec::new();
    };
    session
        .models()
        .iter()
        .find(|entry| entry.provider == model.provider && entry.model == model.model)
        .map(|entry| entry.efforts.clone())
        .unwrap_or_default()
}

pub struct EffortScreen {
    label: String,
    options: Vec<EffortLevel>,
    cursor: usize,
    scroll: usize,
    empty_message: Option<String>,
    done: bool,
}

impl EffortScreen {
    pub fn new(label: String, options: Vec<EffortLevel>, current: Option<EffortLevel>) -> Self {
        let empty_message = options
            .is_empty()
            .then(|| "This model does not support reasoning effort.".to_owned());
        let cursor = current
            .and_then(|current| options.iter().position(|option| *option == current))
            .unwrap_or(0);
        Self {
            label,
            options,
            cursor,
            scroll: 0,
            empty_message,
            done: false,
        }
    }

    fn cap(&self) -> usize {
        self.options.len().min(LIST_MAX)
    }

    fn move_up(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
    }

    fn move_down(&mut self) {
        if self.cursor + 1 >= self.options.len() {
            return;
        }
        self.cursor += 1;
        let cap = self.cap();
        if self.cursor >= self.scroll + cap {
            self.scroll = self.cursor + 1 - cap;
        }
    }

    fn desired_height(&self) -> u16 {
        clamp_u16(self.cap().max(1)).saturating_add(OVERLAY_CHROME)
    }
}

impl Screen for EffortScreen {
    fn placement(&self) -> Placement {
        Placement::Overlay
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, *theme) else {
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
        if let Some(message) = &self.empty_message {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {message}"),
                    theme.muted(),
                ))),
                list_area,
            );
            frame.render_widget(
                Paragraph::new(hint_line(&[(symbols::key::ESC, "close")], *theme)),
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
                let style = if selected { theme.key() } else { theme.base() };
                selection_row(
                    *theme,
                    selected,
                    width,
                    vec![Span::styled(effort.as_str().to_owned(), style)],
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
                *theme,
            )),
            hint_area,
        );
    }

    fn handle_input(&mut self, event: &InputEvent, session: &mut dyn Session) -> InputOutcome {
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        if keymap::ctrl_key(key).is_some() {
            return InputOutcome::Handled(if keymap::ctrl_key(key) == Some('c') {
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
                let Some(effort) = self.options.get(self.cursor).copied() else {
                    return InputOutcome::Handled(ScreenOutcome::Close);
                };
                self.done = true;
                let mut target = session.current_model().cloned().expect("model selected");
                target.effort = Some(effort);
                ScreenOutcome::Effect(CommandEffect::Dispatch(vec![Op::SelectModel { target }]))
            }
            _ => ScreenOutcome::Continue,
        };
        InputOutcome::Handled(outcome)
    }

    fn tick(&mut self) -> ScreenOutcome {
        if self.done {
            ScreenOutcome::Close
        } else {
            ScreenOutcome::Continue
        }
    }
}

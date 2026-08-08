use goat_protocol::ToolCallId;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use goat_command::{
    Theme,
    overlay::{centered_rect, hint_line, overlay_frame, overlay_layout_plain},
    symbols,
    wrap::wrap_line,
};

const SHEET_W: u16 = 96;
const MIN_BODY_ROWS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Review,
    Rejecting,
}

pub struct PlanScreen {
    pub(crate) call: ToolCallId,
    path: String,
    plan: String,
    stage: Stage,
    feedback: String,
    scroll: usize,
    body_rows: usize,
    done: bool,
}

impl PlanScreen {
    pub fn new(call: ToolCallId, plan: String, path: String) -> Self {
        Self {
            call,
            path,
            plan,
            stage: Stage::Review,
            feedback: String::new(),
            scroll: 0,
            body_rows: MIN_BODY_ROWS,
            done: false,
        }
    }

    pub fn rejecting(&self) -> bool {
        self.stage == Stage::Rejecting
    }

    pub fn begin_reject(&mut self) {
        self.stage = Stage::Rejecting;
    }

    pub fn cancel_reject(&mut self) -> bool {
        if self.stage == Stage::Rejecting {
            self.stage = Stage::Review;
            self.feedback.clear();
            true
        } else {
            false
        }
    }

    pub fn push_feedback(&mut self, ch: char) {
        self.feedback.push(ch);
    }

    pub fn pop_feedback(&mut self) {
        self.feedback.pop();
    }

    pub fn submit_reject(&mut self) -> String {
        std::mem::take(&mut self.feedback)
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.scroll = self.scroll.saturating_sub(rows);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll = self.scroll.saturating_add(rows);
    }

    pub fn page(&self) -> usize {
        self.body_rows.max(1)
    }

    fn body_lines(&self, theme: Theme, width: u16) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for raw in self.plan.lines() {
            let line = Line::from(Span::styled(raw.to_owned(), theme.text()));
            out.extend(wrap_line(&line, width));
        }
        out
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: Theme) {
        let height = area.height.min(area.height.max(1));
        let rect = centered_rect(area, SHEET_W, height);
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        let (body_area, hint_area) = overlay_layout_plain(inner);
        let reserved = u16::from(self.stage == Stage::Rejecting) * 2 + 2;
        let text_h = body_area.height.saturating_sub(reserved);
        self.body_rows = usize::from(text_h).max(1);

        let mut lines = vec![
            Line::from(Span::styled(
                format!(" plan · {}", self.path),
                theme.hint_key(),
            )),
            Line::default(),
        ];
        let wrapped = self.body_lines(theme, body_area.width.saturating_sub(1));
        let max_scroll = wrapped.len().saturating_sub(self.body_rows);
        self.scroll = self.scroll.min(max_scroll);
        lines.extend(wrapped.into_iter().skip(self.scroll).take(self.body_rows));

        if self.stage == Stage::Rejecting {
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::styled(" changes: ", theme.hint_key()),
                Span::styled(self.feedback.clone(), theme.text()),
                Span::styled("▏", theme.hint_key()),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), body_area);

        let hints: &[(&str, &str)] = if self.stage == Stage::Rejecting {
            &[
                (symbols::key::ENTER, "send changes"),
                (symbols::key::ESC, "back"),
            ]
        } else {
            &[
                ("a", "approve"),
                ("r", "request changes"),
                ("pgup pgdn", "scroll"),
                (symbols::key::ESC, "later"),
            ]
        };
        frame.render_widget(Paragraph::new(hint_line(hints, theme)), hint_area);
    }
}

impl goat_command::Screen for PlanScreen {
    fn placement(&self) -> goat_command::Placement {
        goat_command::Placement::Overlay
    }

    fn captures_text(&self) -> bool {
        self.rejecting()
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        PlanScreen::render(self, frame, area, *theme);
    }

    fn handle_input(
        &mut self,
        event: &crossterm::event::Event,
        _session: &mut dyn goat_command::Session,
    ) -> goat_command::InputOutcome {
        use crossterm::event::{Event as InputEvent, KeyCode, KeyModifiers};
        use goat_command::{CommandEffect, InputOutcome, ScreenOutcome};
        if let InputEvent::Paste(text) = event {
            if self.rejecting() {
                for ch in text.chars() {
                    self.push_feedback(ch);
                }
                return InputOutcome::Handled(ScreenOutcome::Continue);
            }
            return InputOutcome::Ignored;
        }
        let InputEvent::Key(key) = event else {
            return InputOutcome::Ignored;
        };
        let outcome = if self.rejecting() {
            match key.code {
                KeyCode::Esc => {
                    self.cancel_reject();
                    ScreenOutcome::Continue
                }
                KeyCode::Backspace => {
                    self.pop_feedback();
                    ScreenOutcome::Continue
                }
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.push_feedback(ch);
                    ScreenOutcome::Continue
                }
                KeyCode::Enter => {
                    let call = self.call;
                    let feedback = self.submit_reject();
                    self.done = true;
                    ScreenOutcome::Effect(CommandEffect::Dispatch(vec![
                        goat_protocol::Op::ResolvePlan {
                            call,
                            decision: goat_protocol::PlanDecision::Reject { feedback },
                        },
                    ]))
                }
                _ => ScreenOutcome::Continue,
            }
        } else {
            match key.code {
                KeyCode::Char('a') => {
                    self.done = true;
                    ScreenOutcome::Effect(CommandEffect::Dispatch(vec![
                        goat_protocol::Op::ResolvePlan {
                            call: self.call,
                            decision: goat_protocol::PlanDecision::Approve {},
                        },
                    ]))
                }
                KeyCode::Char('r') => {
                    self.begin_reject();
                    ScreenOutcome::Continue
                }
                KeyCode::PageUp => {
                    self.scroll_up(self.page());
                    ScreenOutcome::Continue
                }
                KeyCode::PageDown => {
                    self.scroll_down(self.page());
                    ScreenOutcome::Continue
                }
                KeyCode::Up => {
                    self.scroll_up(1);
                    ScreenOutcome::Continue
                }
                KeyCode::Down => {
                    self.scroll_down(1);
                    ScreenOutcome::Continue
                }
                KeyCode::Esc => ScreenOutcome::Close,
                _ => ScreenOutcome::Continue,
            }
        };
        InputOutcome::Handled(outcome)
    }

    fn on_event(
        &mut self,
        event: &goat_protocol::Event,
        _session: &mut dyn goat_command::Session,
    ) -> goat_command::ScreenOutcome {
        if matches!(
            event,
            goat_protocol::Event::ModeChanged { mode, .. } if !mode.is_plan()
        ) {
            goat_command::ScreenOutcome::Close
        } else {
            goat_command::ScreenOutcome::Continue
        }
    }

    fn tick(&mut self) -> goat_command::ScreenOutcome {
        if self.done {
            goat_command::ScreenOutcome::Close
        } else {
            goat_command::ScreenOutcome::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::ToolCallId;

    use super::PlanScreen;

    fn sheet() -> PlanScreen {
        PlanScreen::new(
            ToolCallId(1),
            "# Plan\n\nstep one\n".to_owned(),
            "/plans/1-demo.md".to_owned(),
        )
    }

    #[test]
    fn starts_in_review() {
        assert!(!sheet().rejecting());
    }

    #[test]
    fn reject_stage_collects_feedback() {
        let mut sheet = sheet();
        sheet.begin_reject();
        assert!(sheet.rejecting());
        sheet.push_feedback('h');
        sheet.push_feedback('i');
        sheet.push_feedback('x');
        sheet.pop_feedback();
        assert_eq!(sheet.submit_reject(), "hi");
    }

    #[test]
    fn cancelling_reject_clears_feedback_and_returns_to_review() {
        let mut sheet = sheet();
        sheet.begin_reject();
        sheet.push_feedback('x');
        assert!(sheet.cancel_reject());
        assert!(!sheet.rejecting());
        sheet.begin_reject();
        assert_eq!(sheet.submit_reject(), "");
    }

    #[test]
    fn cancel_reject_is_a_noop_in_review() {
        let mut sheet = sheet();
        assert!(!sheet.cancel_reject());
    }

    #[test]
    fn scroll_never_goes_below_zero() {
        let mut sheet = sheet();
        sheet.scroll_up(10);
        sheet.scroll_down(3);
        sheet.scroll_up(99);
        sheet.scroll_down(1);
        assert_eq!(sheet.scroll, 1);
    }
}

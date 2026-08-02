use goat_protocol::ToolCallId;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    overlay::{centered_rect, hint_line, overlay_frame, overlay_layout_plain},
    symbols,
    theme::Theme,
    wrap::wrap_line,
};

const SHEET_W: u16 = 96;
const MIN_BODY_ROWS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Review,
    Rejecting,
}

pub(crate) struct PlanSheet {
    pub(crate) call: ToolCallId,
    path: String,
    plan: String,
    stage: Stage,
    feedback: String,
    scroll: usize,
    body_rows: usize,
}

impl PlanSheet {
    pub(crate) fn new(call: ToolCallId, plan: String, path: String) -> Self {
        Self {
            call,
            path,
            plan,
            stage: Stage::Review,
            feedback: String::new(),
            scroll: 0,
            body_rows: MIN_BODY_ROWS,
        }
    }

    pub(crate) fn rejecting(&self) -> bool {
        self.stage == Stage::Rejecting
    }

    pub(crate) fn begin_reject(&mut self) {
        self.stage = Stage::Rejecting;
    }

    pub(crate) fn cancel_reject(&mut self) -> bool {
        if self.stage == Stage::Rejecting {
            self.stage = Stage::Review;
            self.feedback.clear();
            true
        } else {
            false
        }
    }

    pub(crate) fn push_feedback(&mut self, ch: char) {
        self.feedback.push(ch);
    }

    pub(crate) fn pop_feedback(&mut self) {
        self.feedback.pop();
    }

    pub(crate) fn submit_reject(&mut self) -> String {
        std::mem::take(&mut self.feedback)
    }

    pub(crate) fn scroll_up(&mut self, rows: usize) {
        self.scroll = self.scroll.saturating_sub(rows);
    }

    pub(crate) fn scroll_down(&mut self, rows: usize) {
        self.scroll = self.scroll.saturating_add(rows);
    }

    pub(crate) fn page(&self) -> usize {
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

    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect, theme: Theme) {
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

#[cfg(test)]
mod tests {
    use goat_protocol::ToolCallId;

    use super::PlanSheet;

    fn sheet() -> PlanSheet {
        PlanSheet::new(
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

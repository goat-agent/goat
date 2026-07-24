use goat_protocol::AskQuestion;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_normalization::UnicodeNormalization;
use unicode_width::UnicodeWidthStr;

use crate::{
    layout::LIST_MAX,
    overlay::{ask_sheet_frame, clamp_u16, hint_line},
    symbols,
    theme::Theme,
};

const ASK_BORDER_ROWS: u16 = 2;
const ASK_HEADER_ROWS: u16 = 1;
const ASK_QUESTION_ROWS: u16 = 1;
const ASK_GAP_ROWS: u16 = 2;
const ASK_HINT_ROWS: u16 = 1;
const ASK_REVIEW_HEADER_ROWS: u16 = 1;
const ASK_REVIEW_HINT_ROWS: u16 = 1;
const ASK_REVIEW_ROWS_PER_QUESTION: u16 = 2;

pub enum AskOutcome {
    NoOp,
    Pending,
    Submit(Vec<String>),
}

fn scroll_input(input: &str, avail: usize) -> (String, usize) {
    let total = UnicodeWidthStr::width(input);
    if total < avail {
        return (input.to_owned(), 0);
    }
    let target = avail.saturating_sub(1).max(1);
    let mut acc = 0usize;
    let mut start_byte = input.len();
    for (i, ch) in input.char_indices().rev() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > target {
            break;
        }
        acc += w;
        start_byte = i;
    }
    let scrolled = total - acc;
    (input[start_byte..].to_owned(), scrolled)
}

pub struct AskPicker {
    pub questions: Vec<AskQuestion>,
    pub cursor: usize,
    current_q: usize,
    answers: Vec<Option<String>>,
    saved: Vec<Option<(Vec<bool>, String, usize)>>,
    selected: Vec<bool>,
    typing: bool,
    input: String,
    confirming: bool,
}

impl AskPicker {
    pub fn new(questions: Vec<AskQuestion>) -> Self {
        let count = questions.len();
        let first_options = questions.first().map_or(0, |q| q.options.len());
        Self {
            questions,
            cursor: 0,
            current_q: 0,
            answers: vec![None; count],
            saved: vec![None; count],
            selected: vec![false; first_options],
            typing: false,
            input: String::new(),
            confirming: false,
        }
    }

    fn is_multi(&self) -> bool {
        self.questions[self.current_q].multiple
    }

    pub fn toggle(&mut self) {
        if self.confirming || !self.is_multi() {
            return;
        }
        if self.cursor < self.questions[self.current_q].options.len()
            && let Some(slot) = self.selected.get_mut(self.cursor)
        {
            *slot = !*slot;
        }
    }

    fn join_selection(&self) -> String {
        let mut parts: Vec<String> = self.questions[self.current_q]
            .options
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected.get(*i).copied().unwrap_or(false))
            .map(|(_, opt)| opt.label.clone())
            .collect();
        let trimmed = self.input.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_owned());
        }
        parts.join(", ")
    }

    fn reset_selection(&mut self, q_idx: usize) {
        let len = self.questions[q_idx].options.len();
        self.selected = vec![false; len];
    }

    pub fn is_confirming(&self) -> bool {
        self.confirming
    }

    pub fn is_typing(&self) -> bool {
        self.typing
    }

    pub fn wants_toggle(&self) -> bool {
        !self.confirming
            && !self.typing
            && self.is_multi()
            && self.cursor < self.questions[self.current_q].options.len()
    }

    pub fn insert_str(&mut self, text: &str) {
        let on_input_row = self.cursor == self.questions[self.current_q].options.len();
        if self.typing || on_input_row {
            self.typing = true;
            for ch in text.nfc() {
                self.input.push(ch);
            }
        }
    }

    pub fn move_up(&mut self) {
        if self.confirming {
            return;
        }
        if self.typing {
            self.typing = false;
        }
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.confirming {
            return;
        }
        if self.typing {
            self.typing = false;
        }
        let max = self.questions[self.current_q].options.len();
        if self.cursor < max {
            self.cursor += 1;
        }
    }

    pub fn on_char(&mut self, ch: char) {
        if self.confirming {
            return;
        }
        let on_input_row = self.cursor == self.questions[self.current_q].options.len();
        if self.typing || on_input_row {
            self.typing = true;
            self.input.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        if self.typing {
            self.input.pop();
        }
    }

    pub fn choose(&mut self) -> AskOutcome {
        if self.confirming {
            return self.finish();
        }
        let type_own_idx = self.questions[self.current_q].options.len();
        if self.is_multi() {
            if self.cursor < type_own_idx {
                self.toggle();
                return AskOutcome::Pending;
            }
            if !self.input.is_empty() {
                self.typing = false;
            }
            let answer = self.join_selection();
            return self.record_answer(answer);
        }
        if self.cursor == type_own_idx {
            if !self.input.is_empty() {
                let answer = self.input.clone();
                self.typing = false;
                return self.record_answer(answer);
            }
            self.typing = true;
            return AskOutcome::Pending;
        }
        if let Some(opt) = self.questions[self.current_q].options.get(self.cursor) {
            let answer = opt.label.clone();
            return self.record_answer(answer);
        }
        AskOutcome::NoOp
    }

    pub fn skip(&mut self) -> AskOutcome {
        if self.confirming {
            return AskOutcome::NoOp;
        }
        self.typing = false;
        self.input.clear();
        self.advance()
    }

    pub fn go_back(&mut self) {
        if self.confirming {
            self.confirming = false;
            let last = self.questions.len().saturating_sub(1);
            self.current_q = last;
            self.restore_cursor(last);
            return;
        }
        if self.typing {
            self.typing = false;
            return;
        }
        if self.current_q > 0 {
            self.current_q -= 1;
            self.restore_cursor(self.current_q);
        }
    }

    pub fn desired_height(&self) -> u16 {
        if self.confirming {
            let rows = clamp_u16(self.questions.len())
                .saturating_mul(ASK_REVIEW_ROWS_PER_QUESTION)
                .saturating_add(ASK_REVIEW_HEADER_ROWS)
                .saturating_add(ASK_REVIEW_HINT_ROWS)
                .saturating_add(ASK_GAP_ROWS);
            rows.saturating_add(ASK_BORDER_ROWS)
        } else {
            let q = &self.questions[self.current_q];
            let rows = clamp_u16(q.options.len() + 1).min(clamp_u16(LIST_MAX));
            rows.saturating_add(ASK_BORDER_ROWS)
                .saturating_add(ASK_HEADER_ROWS)
                .saturating_add(ASK_QUESTION_ROWS)
                .saturating_add(ASK_GAP_ROWS)
                .saturating_add(ASK_HINT_ROWS)
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let panel_h = self.desired_height();
        let [_, outer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(panel_h)]).areas(area);
        let Some(inner) = ask_sheet_frame(frame, outer, theme) else {
            return;
        };
        if self.confirming {
            self.render_confirm(frame, inner, theme);
        } else {
            self.render_question(frame, inner, theme);
        }
    }

    fn restore_cursor(&mut self, q_idx: usize) {
        self.typing = false;
        self.input.clear();
        self.cursor = 0;
        self.reset_selection(q_idx);
        let Some((selected, input, cursor)) = self.saved.get(q_idx).cloned().flatten() else {
            return;
        };
        if selected.len() == self.questions[q_idx].options.len() {
            self.selected = selected;
        }
        self.input = input;
        self.cursor = cursor.min(self.questions[q_idx].options.len());
    }

    fn record_answer(&mut self, answer: String) -> AskOutcome {
        self.answers[self.current_q] = Some(answer);
        self.saved[self.current_q] = Some((self.selected.clone(), self.input.clone(), self.cursor));
        self.advance()
    }

    fn advance(&mut self) -> AskOutcome {
        if self.current_q + 1 < self.questions.len() {
            self.current_q += 1;
            self.cursor = 0;
            self.typing = false;
            self.input.clear();
            self.reset_selection(self.current_q);
            AskOutcome::Pending
        } else if self.questions.len() > 1 {
            self.confirming = true;
            AskOutcome::Pending
        } else {
            self.finish()
        }
    }

    fn finish(&self) -> AskOutcome {
        let answers = self
            .answers
            .iter()
            .map(|a| a.clone().unwrap_or_default())
            .collect();
        AskOutcome::Submit(answers)
    }

    fn render_question(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let [header, question, _, list, _, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        self.render_header(frame, header, theme);
        self.render_question_text(frame, question, theme);
        self.render_list(frame, list, theme);
        self.render_hint(frame, hint, theme);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let total = self.questions.len();
        let title = if total > 1 {
            format!(" Question {}/{}", self.current_q + 1, total)
        } else {
            " Answer needed".to_owned()
        };
        let dots = if total > 1 {
            let dot_str: Vec<&str> = (0..total)
                .map(|i| {
                    if i <= self.current_q {
                        symbols::ui::DOT_FULL
                    } else {
                        symbols::ui::DOT_EMPTY
                    }
                })
                .collect();
            format!("  {}", dot_str.join(" "))
        } else {
            String::new()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(title, theme.muted()),
                Span::styled(dots, theme.accent()),
            ])),
            area,
        );
    }

    fn render_question_text(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let q = &self.questions[self.current_q];
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", q.question),
                theme.text(),
            ))),
            area,
        );
    }

    fn render_list(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let q = &self.questions[self.current_q];
        let multi = q.multiple;
        let width = usize::from(area.width);
        let type_own_idx = q.options.len();
        let mut lines: Vec<Line> = Vec::new();
        for (i, opt) in q.options.iter().enumerate() {
            let selected = i == self.cursor;
            let right = opt
                .description
                .as_deref()
                .map(|d| Span::styled(d.to_owned(), theme.muted()));
            let mut left: Vec<Span> = Vec::new();
            if multi {
                let glyph = if self.selected.get(i).copied().unwrap_or(false) {
                    symbols::ui::DOT_FULL
                } else {
                    symbols::ui::DOT_EMPTY
                };
                left.push(Span::styled(format!("{glyph} "), theme.accent()));
            }
            left.push(Span::styled(opt.label.clone(), theme.text()));
            lines.push(Self::ask_row(selected, width, left, right, theme));
        }
        let input_selected = self.cursor == type_own_idx;
        let caret = if input_selected {
            Span::styled(format!(" {} ", symbols::ui::CARET), theme.accent())
        } else {
            Span::raw("   ")
        };
        let avail = width.saturating_sub(3).max(1);
        let (visible, scrolled) = scroll_input(&self.input, avail);
        let body = if self.typing || !self.input.is_empty() {
            Span::styled(visible, theme.text())
        } else {
            Span::styled("Type custom answer".to_owned(), theme.muted())
        };
        let used = 3 + UnicodeWidthStr::width(body.content.as_ref());
        let pad = width.saturating_sub(used);
        lines.push(Line::from(vec![caret, body, Span::raw(" ".repeat(pad))]));
        frame.render_widget(Paragraph::new(lines), area);
        if self.typing && input_selected {
            let row_y = area.y + clamp_u16(type_own_idx);
            let shown = UnicodeWidthStr::width(self.input.as_str()).saturating_sub(scrolled);
            let col = 3 + shown.min(avail);
            let x = area.x + clamp_u16(col);
            frame.set_cursor_position((x.min(area.right().saturating_sub(1)), row_y));
        }
    }

    fn ask_row<'a>(
        selected: bool,
        inner_width: usize,
        left_spans: Vec<Span<'a>>,
        right_span: Option<Span<'a>>,
        theme: Theme,
    ) -> Line<'a> {
        let caret = if selected {
            Span::styled(format!(" {} ", symbols::ui::CARET), theme.accent())
        } else {
            Span::raw("   ")
        };
        let left_w: usize = left_spans.iter().map(|s| s.content.width()).sum();
        let right_w = right_span.as_ref().map_or(0, |s| s.content.width());
        let caret_w = 3usize;
        let pad = inner_width
            .saturating_sub(caret_w + left_w + right_w + usize::from(right_span.is_some()));
        let mut spans = vec![caret];
        spans.extend(left_spans);
        if let Some(right) = right_span {
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(right);
        } else {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        Line::from(spans)
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let [title, _, list, _, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" Review answers", theme.muted()))),
            title,
        );
        let mut lines: Vec<Line> = Vec::new();
        for (i, q) in self.questions.iter().enumerate() {
            let answer = self.answers[i].as_deref().filter(|s| !s.is_empty());
            let (answer_text, answer_style) = match answer {
                Some(a) => (a.to_owned(), theme.text()),
                None => ("—".to_owned(), theme.muted()),
            };
            lines.push(Line::from(Span::styled(
                format!(" {}", q.question),
                theme.muted(),
            )));
            lines.push(Line::from(Span::styled(
                format!("   {answer_text}"),
                answer_style,
            )));
        }
        frame.render_widget(Paragraph::new(lines), list);
        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ENTER, "submit"),
                    (symbols::key::ARROW_LEFT, "edit"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )),
            hint,
        );
    }

    fn render_hint(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let total = self.questions.len();
        let q = &self.questions[self.current_q];
        let hint = if self.typing {
            hint_line(
                &[
                    (symbols::key::ENTER, "confirm"),
                    (symbols::key::ESC, "back"),
                ],
                theme,
            )
        } else if q.multiple {
            let submit = if total > 1 { "next" } else { "submit" };
            hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "move"),
                    ("space", "toggle"),
                    (symbols::key::ENTER, submit),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )
        } else if total > 1 {
            hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "choose"),
                    (symbols::key::ENTER, "next"),
                    (symbols::key::ARROW_RIGHT, "skip"),
                    (symbols::key::ARROW_LEFT, "back"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )
        } else if q.options.is_empty() {
            hint_line(
                &[
                    (symbols::key::ENTER, "confirm"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )
        } else {
            hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "choose"),
                    (symbols::key::ENTER, "select"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )
        };
        frame.render_widget(Paragraph::new(hint), area);
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::{AskOption, AskQuestion};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Color};

    use super::{AskOutcome, AskPicker};
    use crate::theme::Theme;

    fn option(label: &str, description: &str) -> AskOption {
        AskOption {
            label: label.to_owned(),
            description: if description.is_empty() {
                None
            } else {
                Some(description.to_owned())
            },
        }
    }

    fn question(text: &str, options: Vec<AskOption>) -> AskQuestion {
        AskQuestion {
            question: text.to_owned(),
            options,
            multiple: false,
        }
    }

    fn multi_question(text: &str, options: Vec<AskOption>) -> AskQuestion {
        AskQuestion {
            question: text.to_owned(),
            options,
            multiple: true,
        }
    }

    fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn single_option_submits_label() {
        let mut picker = AskPicker::new(vec![question("Deploy?", vec![option("yes", "")])]);
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["yes"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn custom_input_requires_text_before_submit() {
        let mut picker = AskPicker::new(vec![question("Deploy?", Vec::new())]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        assert!(picker.is_typing());
        picker.insert_str("canary");
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["canary"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn multi_question_enters_review_before_submit() {
        let mut picker = AskPicker::new(vec![
            question("Deploy?", vec![option("yes", "")]),
            question("Migrate?", vec![option("no", "")]),
        ]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        assert!(!picker.is_confirming());
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        assert!(picker.is_confirming());
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["yes", "no"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn go_back_restores_custom_answer() {
        let mut picker = AskPicker::new(vec![
            question("Branch?", Vec::new()),
            question("Deploy?", vec![option("yes", "")]),
        ]);
        picker.insert_str("feature/x");
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.go_back();
        assert_eq!(picker.cursor, 0);
        picker.insert_str("-next");
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["feature/x-next", "yes"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn skip_records_empty_answer() {
        let mut picker = AskPicker::new(vec![question("Deploy?", vec![option("yes", "")])]);
        match picker.skip() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec![""]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn renders_normal_prompt_without_panel_background_text() {
        let picker = AskPicker::new(vec![question(
            "Which deployment target should I use?",
            vec![option("production", "stable public release")],
        )]);
        let mut terminal = Terminal::new(TestBackend::new(64, 12)).unwrap();
        terminal
            .draw(|frame| picker.render(frame, Rect::new(0, 0, 64, 12), Theme::light()))
            .unwrap();
        let all = (0..12)
            .map(|y| row(&terminal, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Answer needed"));
        assert!(all.contains("Which deployment target should I use?"));
        assert!(all.contains("production"));
        assert!(all.contains("Type custom answer"));
        assert!(all.contains("choose"));
        let buffer = terminal.backend().buffer();
        let cell = &buffer[(2, 5)];
        assert_ne!(cell.style().bg, Some(Color::Rgb(0xee, 0xee, 0xf0)));
        assert_ne!(cell.style().bg, Some(Color::Black));
    }

    #[test]
    fn renders_review_prompt() {
        let mut picker = AskPicker::new(vec![
            question("Deployment target", vec![option("production", "")]),
            question("Run migrations now?", Vec::new()),
        ]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        assert!(matches!(picker.skip(), AskOutcome::Pending));
        let mut terminal = Terminal::new(TestBackend::new(64, 12)).unwrap();
        terminal
            .draw(|frame| picker.render(frame, Rect::new(0, 0, 64, 12), Theme::dark()))
            .unwrap();
        let all = (0..12)
            .map(|y| row(&terminal, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Review answers"));
        assert!(all.contains("Deployment target"));
        assert!(all.contains("production"));
        assert!(all.contains("Run migrations now?"));
        assert!(all.contains("submit"));
    }

    #[test]
    fn multi_select_joins_toggled_labels() {
        let mut picker = AskPicker::new(vec![multi_question(
            "Colors?",
            vec![option("red", ""), option("green", ""), option("blue", "")],
        )]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.move_down();
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.move_down();
        picker.move_down();
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["red, green"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn multi_select_allows_empty_submit() {
        let mut picker = AskPicker::new(vec![multi_question(
            "Colors?",
            vec![option("red", ""), option("green", "")],
        )]);
        picker.move_down();
        picker.move_down();
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec![""]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn multi_select_toggle_off_removes_label() {
        let mut picker = AskPicker::new(vec![multi_question(
            "Colors?",
            vec![option("red", ""), option("green", "")],
        )]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.move_down();
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.move_up();
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.move_down();
        picker.move_down();
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["green"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn multi_select_appends_custom_input() {
        let mut picker = AskPicker::new(vec![multi_question(
            "Colors?",
            vec![option("red", ""), option("green", "")],
        )]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.move_down();
        picker.move_down();
        picker.insert_str("teal");
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["red, teal"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn go_back_restores_multi_select_with_comma_label() {
        let mut picker = AskPicker::new(vec![
            multi_question(
                "Colors?",
                vec![option("red, crimson", ""), option("green", "")],
            ),
            question("Deploy?", vec![option("yes", "")]),
        ]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.move_down();
        picker.move_down();
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        picker.go_back();
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        match picker.choose() {
            AskOutcome::Submit(answers) => assert_eq!(answers, vec!["red, crimson", "yes"]),
            AskOutcome::NoOp | AskOutcome::Pending => panic!("expected submit"),
        }
    }

    #[test]
    fn renders_multi_select_checkboxes() {
        let mut picker = AskPicker::new(vec![multi_question(
            "Colors?",
            vec![option("red", ""), option("green", "")],
        )]);
        assert!(matches!(picker.choose(), AskOutcome::Pending));
        let mut terminal = Terminal::new(TestBackend::new(64, 12)).unwrap();
        terminal
            .draw(|frame| picker.render(frame, Rect::new(0, 0, 64, 12), Theme::dark()))
            .unwrap();
        let all = (0..12)
            .map(|y| row(&terminal, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains(super::symbols::ui::DOT_FULL));
        assert!(all.contains(super::symbols::ui::DOT_EMPTY));
        assert!(all.contains("toggle"));
    }
}

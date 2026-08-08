use goat_protocol::ConversationSummary;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use goat_command::{
    Theme,
    layout::{LIST_MAX, OVERLAY_CHROME_PLAIN, OVERLAY_W},
    overlay::{
        centered_rect, clamp_u16, hint_line, overlay_frame, overlay_layout_plain, selection_row,
    },
    symbols,
};

enum ConversationOutcome {
    NoOp,
    Selected(i64),
}

pub struct ResumeScreen {
    conversations: Vec<ConversationSummary>,
    cursor: usize,
    scroll: usize,
    index: Option<usize>,
    loading: bool,
    started: bool,
    done: bool,
}

impl ResumeScreen {
    pub fn new(conversations: Vec<ConversationSummary>) -> Self {
        Self {
            conversations,
            cursor: 0,
            scroll: 0,
            index: None,
            loading: true,
            started: false,
            done: false,
        }
    }

    pub fn indexed(index: usize) -> Self {
        let mut screen = Self::new(Vec::new());
        screen.index = Some(index);
        screen
    }

    fn cap(&self) -> usize {
        self.conversations.len().min(LIST_MAX)
    }

    fn visible_items(&self) -> usize {
        let cap = self.cap();
        if self.conversations.len() > LIST_MAX {
            cap.saturating_sub(2)
        } else {
            cap
        }
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
        if self.cursor + 1 >= self.conversations.len() {
            return;
        }
        self.cursor += 1;
        let vis = self.visible_items();
        if self.cursor >= self.scroll + vis {
            self.scroll = self.cursor + 1 - vis;
        }
    }

    fn choose(&self) -> ConversationOutcome {
        self.conversations
            .get(self.cursor)
            .map_or(ConversationOutcome::NoOp, |conversation| {
                ConversationOutcome::Selected(conversation.id)
            })
    }

    pub fn desired_height(&self) -> u16 {
        clamp_u16(self.cap().max(1)).saturating_add(OVERLAY_CHROME_PLAIN)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        let (list_area, hint_area) = overlay_layout_plain(inner);

        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height).max(1);
        let scroll = self.scroll.min(self.cursor);
        let mut lines: Vec<Line> = Vec::new();
        if self.conversations.is_empty() {
            let message = if self.loading {
                format!(" loading conversations {}", symbols::ui::ELLIPSIS)
            } else {
                " no past conversations in this directory".to_owned()
            };
            lines.push(Line::from(Span::styled(message, theme.muted())));
        } else {
            let above_rows = usize::from(scroll > 0);
            let budget = rows.saturating_sub(above_rows);
            let remaining = self.conversations.len().saturating_sub(scroll);
            let has_below = remaining > budget;
            let take = if has_below {
                budget.saturating_sub(1)
            } else {
                budget.min(remaining)
            };

            if scroll > 0 {
                lines.push(Line::from(Span::styled(
                    format!(" {} {} more", symbols::ui::MORE_ABOVE, scroll),
                    theme.muted(),
                )));
            }
            for (idx, conversation) in self
                .conversations
                .iter()
                .enumerate()
                .skip(scroll)
                .take(take)
            {
                let selected = idx == self.cursor;
                let title_style = if selected { theme.key() } else { theme.base() };
                let mut left = vec![Span::styled(format!("{}. ", idx + 1), theme.muted())];
                if conversation.live {
                    left.push(Span::styled(
                        format!("{} ", symbols::ui::DOT_FULL),
                        theme.key(),
                    ));
                }
                left.push(Span::styled(conversation.title.clone(), title_style));
                let right = Some(Span::styled(conversation.model.clone(), theme.muted()));
                lines.push(selection_row(theme, selected, width, left, right));
            }
            if has_below {
                let hidden = self.conversations.len() - scroll - take;
                lines.push(Line::from(Span::styled(
                    format!(" {} {} more", symbols::ui::MORE_BELOW, hidden),
                    theme.muted(),
                )));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "navigate"),
                    (symbols::key::ENTER, "resume"),
                    (symbols::key::ESC, "close"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

impl goat_command::Screen for ResumeScreen {
    fn placement(&self) -> goat_command::Placement {
        if self.index.is_some() {
            goat_command::Placement::Hidden
        } else {
            goat_command::Placement::Overlay
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if self.index.is_none() {
            ResumeScreen::render(self, frame, area, *theme);
        }
    }

    fn handle_input(
        &mut self,
        event: &crossterm::event::Event,
        _session: &mut dyn goat_command::Session,
    ) -> goat_command::InputOutcome {
        use crossterm::event::{Event as InputEvent, KeyCode};
        use goat_command::{CommandEffect, InputOutcome, ScreenOutcome};
        if self.index.is_some() {
            return InputOutcome::Ignored;
        }
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
            KeyCode::Enter => match self.choose() {
                ConversationOutcome::NoOp => ScreenOutcome::Continue,
                ConversationOutcome::Selected(conversation_id) => {
                    self.done = true;
                    ScreenOutcome::Effect(CommandEffect::Dispatch(vec![
                        goat_protocol::Op::Resume { conversation_id },
                    ]))
                }
            },
            _ => ScreenOutcome::Continue,
        };
        InputOutcome::Handled(outcome)
    }

    fn on_event(
        &mut self,
        event: &goat_protocol::Event,
        session: &mut dyn goat_command::Session,
    ) -> goat_command::ScreenOutcome {
        let goat_protocol::Event::ConversationsListed { conversations } = event else {
            return goat_command::ScreenOutcome::Continue;
        };
        self.loading = false;
        if let Some(index) = self.index {
            self.done = true;
            return if let Some(conversation) = conversations.get(index) {
                goat_command::ScreenOutcome::Effect(goat_command::CommandEffect::Dispatch(vec![
                    goat_protocol::Op::Resume {
                        conversation_id: conversation.id,
                    },
                ]))
            } else {
                session.notify(
                    goat_protocol::NotifyKind::Error,
                    format!("no conversation #{}", index + 1),
                );
                goat_command::ScreenOutcome::Close
            };
        }
        self.conversations.clone_from(conversations);
        self.cursor = 0;
        self.scroll = 0;
        goat_command::ScreenOutcome::Continue
    }

    fn tick(&mut self) -> goat_command::ScreenOutcome {
        if self.done {
            return goat_command::ScreenOutcome::Close;
        }
        if self.started {
            goat_command::ScreenOutcome::Continue
        } else {
            self.started = true;
            goat_command::ScreenOutcome::Effect(goat_command::CommandEffect::Dispatch(vec![
                goat_protocol::Op::ListConversations {},
            ]))
        }
    }
}

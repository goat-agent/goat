use goat_protocol::{RewindPoint, RewindScope};
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

pub enum RewindOutcome {
    NoOp,
    Close,
    Selected {
        checkpoint_id: i64,
        scope: RewindScope,
    },
}

#[derive(Clone, Copy)]
enum Action {
    CodeAndConversation,
    Conversation,
    Code,
    NeverMind,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::CodeAndConversation => "Restore code and conversation",
            Self::Conversation => "Restore conversation",
            Self::Code => "Restore code",
            Self::NeverMind => "Never mind",
        }
    }

    fn scope(self) -> Option<RewindScope> {
        match self {
            Self::CodeAndConversation => Some(RewindScope::CodeAndConversation),
            Self::Conversation => Some(RewindScope::Conversation),
            Self::Code => Some(RewindScope::Code),
            Self::NeverMind => None,
        }
    }
}

pub struct RewindPicker {
    points: Vec<RewindPoint>,
    cursor: usize,
    actions: Option<(i64, Vec<Action>)>,
}

impl RewindPicker {
    pub fn new(points: Vec<RewindPoint>) -> Self {
        Self {
            points,
            cursor: 0,
            actions: None,
        }
    }

    fn len(&self) -> usize {
        self.actions
            .as_ref()
            .map_or(self.points.len(), |(_, actions)| actions.len())
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.len() {
            self.cursor += 1;
        }
    }

    pub fn enter(&mut self) -> RewindOutcome {
        if let Some((checkpoint_id, actions)) = &self.actions {
            let Some(action) = actions.get(self.cursor).copied() else {
                return RewindOutcome::NoOp;
            };
            return action
                .scope()
                .map_or(RewindOutcome::Close, |scope| RewindOutcome::Selected {
                    checkpoint_id: *checkpoint_id,
                    scope,
                });
        }
        let Some(point) = self.points.get(self.cursor) else {
            return RewindOutcome::NoOp;
        };
        let mut actions = Vec::new();
        if point.code_changes {
            actions.push(Action::CodeAndConversation);
        }
        actions.push(Action::Conversation);
        if point.code_changes {
            actions.push(Action::Code);
        }
        actions.push(Action::NeverMind);
        self.actions = Some((point.checkpoint_id, actions));
        self.cursor = 0;
        RewindOutcome::NoOp
    }

    pub fn escape(&mut self) -> RewindOutcome {
        if self.actions.take().is_some() {
            self.cursor = 0;
            RewindOutcome::NoOp
        } else {
            RewindOutcome::Close
        }
    }

    pub fn desired_height(&self) -> u16 {
        clamp_u16(self.len().min(LIST_MAX).max(1)).saturating_add(OVERLAY_CHROME)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        let (context_area, list_area, hint_area) = overlay_layout(inner);
        let context = if self.actions.is_some() {
            "Choose what to restore"
        } else {
            "Rewind to before a prompt"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {context}"),
                theme.muted(),
            ))),
            context_area,
        );
        let width = usize::from(list_area.width);
        let lines = if let Some((_, actions)) = &self.actions {
            actions
                .iter()
                .enumerate()
                .map(|(index, action)| {
                    selection_row(
                        theme,
                        index == self.cursor,
                        width,
                        vec![Span::styled(
                            action.label(),
                            if index == self.cursor {
                                theme.key()
                            } else {
                                theme.base()
                            },
                        )],
                        None,
                    )
                })
                .collect()
        } else if self.points.is_empty() {
            vec![Line::from(Span::styled(
                " no prompts to rewind",
                theme.muted(),
            ))]
        } else {
            let start = self.cursor.saturating_add(1).saturating_sub(LIST_MAX);
            self.points
                .iter()
                .skip(start)
                .take(LIST_MAX)
                .enumerate()
                .map(|(index, point)| {
                    let index = start + index;
                    let selected = index == self.cursor;
                    let prompt = point
                        .prompt
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("(empty prompt)");
                    let right = point
                        .code_changes
                        .then(|| Span::styled("code", theme.muted()));
                    selection_row(
                        theme,
                        selected,
                        width,
                        vec![
                            Span::styled(format!("{}. ", index + 1), theme.muted()),
                            Span::styled(
                                prompt.to_owned(),
                                if selected { theme.key() } else { theme.base() },
                            ),
                        ],
                        right,
                    )
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), list_area);
        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "navigate"),
                    (symbols::key::ENTER, "select"),
                    (symbols::key::ESC, "back"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

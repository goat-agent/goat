use goat_protocol::ThreadSummary;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    layout::{LIST_MAX, OVERLAY_CHROME_PLAIN, OVERLAY_W},
    overlay::{
        centered_rect, clamp_u16, hint_line, overlay_frame, overlay_layout_plain, selection_row,
    },
    symbols,
    theme::Theme,
};

pub enum ThreadOutcome {
    NoOp,
    Selected(i64),
}

pub struct ThreadPicker {
    threads: Vec<ThreadSummary>,
    cursor: usize,
    scroll: usize,
}

impl ThreadPicker {
    pub fn new(threads: Vec<ThreadSummary>) -> Self {
        Self {
            threads,
            cursor: 0,
            scroll: 0,
        }
    }

    fn cap(&self) -> usize {
        self.threads.len().min(LIST_MAX)
    }

    fn visible_items(&self) -> usize {
        let cap = self.cap();
        if self.threads.len() > LIST_MAX {
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
        if self.cursor + 1 >= self.threads.len() {
            return;
        }
        self.cursor += 1;
        let vis = self.visible_items();
        if self.cursor >= self.scroll + vis {
            self.scroll = self.cursor + 1 - vis;
        }
    }

    pub fn choose(&self) -> ThreadOutcome {
        self.threads
            .get(self.cursor)
            .map_or(ThreadOutcome::NoOp, |thread| {
                ThreadOutcome::Selected(thread.id)
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
        if self.threads.is_empty() {
            lines.push(Line::from(Span::styled(
                " no past conversations in this directory",
                theme.muted(),
            )));
        } else {
            let above_rows = usize::from(scroll > 0);
            let budget = rows.saturating_sub(above_rows);
            let remaining = self.threads.len().saturating_sub(scroll);
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
            for (idx, thread) in self.threads.iter().enumerate().skip(scroll).take(take) {
                let selected = idx == self.cursor;
                let title_style = if selected { theme.key() } else { theme.base() };
                let mut left = vec![Span::styled(format!("{}. ", idx + 1), theme.muted())];
                if thread.live {
                    left.push(Span::styled(
                        format!("{} ", symbols::ui::DOT_FULL),
                        theme.key(),
                    ));
                }
                left.push(Span::styled(thread.title.clone(), title_style));
                let right = Some(Span::styled(thread.model.clone(), theme.muted()));
                lines.push(selection_row(theme, selected, width, left, right));
            }
            if has_below {
                let hidden = self.threads.len() - scroll - take;
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

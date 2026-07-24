use goat_protocol::NotifyKind;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{symbols, theme::Theme, wrap};

pub struct Toast {
    message: String,
    kind: NotifyKind,
    ticks_left: u16,
}

const TOAST_TICKS: u16 = 33;
const TOAST_GAP: u16 = 1;
const MIN_WIDTH: u16 = 12;
const TOAST_MAX_LINES: usize = 3;

impl Toast {
    pub fn new(kind: NotifyKind, message: String) -> Self {
        Self {
            kind,
            message,
            ticks_left: TOAST_TICKS,
        }
    }

    fn icon(&self) -> &'static str {
        match self.kind {
            NotifyKind::Success => symbols::ui::CHECK,
            NotifyKind::Error => symbols::ui::CROSS,
            NotifyKind::Info => symbols::ui::MIDDOT,
        }
    }

    fn icon_style(&self, theme: Theme) -> ratatui::style::Style {
        match self.kind {
            NotifyKind::Success => theme.success(),
            NotifyKind::Error => theme.error(),
            NotifyKind::Info => theme.muted(),
        }
    }
}

pub fn tick(toasts: &mut Vec<Toast>) -> bool {
    let before = toasts.len();
    for t in toasts.iter_mut() {
        t.ticks_left = t.ticks_left.saturating_sub(1);
    }
    toasts.retain(|t| t.ticks_left > 0);
    toasts.len() != before
}

pub fn render(frame: &mut Frame, area: Rect, theme: Theme, toasts: &[Toast]) {
    if toasts.is_empty() || area.width < MIN_WIDTH + 4 || area.height < 3 {
        return;
    }
    let max_width = area.width.saturating_sub(4);
    let text_width = max_width.saturating_sub(6).max(1);
    let mut y = area.y.saturating_add(1);
    for toast in toasts.iter().rev() {
        let body = Line::from(Span::raw(toast.message.clone()));
        let mut wrapped = wrap::wrap_line(&body, text_width);
        let truncated = wrapped.len() > TOAST_MAX_LINES;
        wrapped.truncate(TOAST_MAX_LINES);
        if wrapped.is_empty() {
            wrapped.push(Line::default());
        }
        let height = u16::try_from(wrapped.len()).unwrap_or(1).saturating_add(2);
        if y.saturating_add(height) > area.bottom() {
            break;
        }
        let content_w = wrapped
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        let width = u16::try_from(content_w)
            .unwrap_or(u16::MAX)
            .saturating_add(6)
            .clamp(MIN_WIDTH, max_width);
        let x = area.right().saturating_sub(width).saturating_sub(1);
        let rect = Rect {
            x,
            y,
            width,
            height,
        };
        frame.render_widget(Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(toast.icon_style(theme))
            .style(theme.panel())
            .padding(Padding::horizontal(1));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let mut lines: Vec<Line> = Vec::with_capacity(wrapped.len());
        for (i, mut line) in wrapped.into_iter().enumerate() {
            let icon = if i == 0 {
                Span::styled(toast.icon(), toast.icon_style(theme))
            } else {
                Span::raw(" ")
            };
            if truncated && i + 1 == TOAST_MAX_LINES {
                line.spans
                    .push(Span::styled(symbols::ui::ELLIPSIS, theme.muted()));
            }
            let mut spans = vec![icon, Span::raw(" ")];
            for mut span in line.spans {
                span.style = theme.text().patch(span.style);
                spans.push(span);
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        y = y.saturating_add(height + TOAST_GAP);
    }
}

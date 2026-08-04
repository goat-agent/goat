use std::time::Instant;

use goat_client::Identity;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    layout::{OVERLAY_CHROME_PLAIN, OVERLAY_W},
    overlay::{centered_rect, clamp_u16, overlay_frame, overlay_layout_plain},
    theme::Theme,
};

pub struct StatusRow {
    pub label: &'static str,
    pub value: String,
}

pub struct StatusView<'a> {
    rows: &'a [StatusRow],
}

impl<'a> StatusView<'a> {
    pub fn new(rows: &'a [StatusRow]) -> Self {
        Self { rows }
    }

    pub fn desired_height(&self) -> u16 {
        clamp_u16(self.rows.len() + 1)
            .saturating_add(OVERLAY_CHROME_PLAIN)
            .clamp(8, 32)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        let (body_area, hint_area) = overlay_layout_plain(inner);
        let mut lines = vec![Line::from(vec![Span::styled(" status", theme.accent())])];
        for row in self.rows {
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(format!("{:<10}", row.label), theme.muted()),
                Span::styled(row.value.clone(), theme.base()),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), body_area);
        let _ = hint_area;
    }
}

#[must_use]
pub fn daemon_uptime(started_at: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    age(now_ms.saturating_sub(started_at))
}

#[must_use]
pub fn uptime(started: Instant) -> String {
    age(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX))
}

fn age(millis: i64) -> String {
    let secs = millis.max(0) / 1000;
    match (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[must_use]
pub fn busy_label(sessions: usize, turns: usize) -> String {
    let mut parts = Vec::new();
    if turns > 0 {
        parts.push(format!("{turns} agent turn(s) in flight"));
    }
    if sessions > 0 {
        parts.push(format!("{sessions} live coding session(s)"));
    }
    if parts.is_empty() {
        "idle".to_owned()
    } else {
        parts.join(", ")
    }
}

#[must_use]
pub fn daemon_label(daemon: &Identity) -> String {
    format!(
        "goat {} · pid {} · up {} · {}",
        daemon.version,
        daemon.pid,
        daemon_uptime(daemon.started_at),
        busy_label(daemon.busy.sessions, daemon.busy.turns)
    )
}

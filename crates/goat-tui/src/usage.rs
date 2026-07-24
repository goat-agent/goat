use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use goat_protocol::{AccountEntry, AuthMethod, ModelTarget, RateLimitSnapshot, Usage};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    layout::{OVERLAY_CHROME_PLAIN, OVERLAY_W, format_tokens},
    overlay::{self, centered_rect, clamp_u16, overlay_frame, overlay_layout_plain},
    symbols,
    theme::Theme,
};

const BAR_WIDTH: usize = 12;

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn filled_cells(pct: f32) -> usize {
    let n = (pct / 100.0 * BAR_WIDTH as f32).round();
    (n as usize).min(BAR_WIDTH)
}

fn render_bar_line(
    label: &str,
    pct: f32,
    right_text: &str,
    theme: Theme,
    area_width: usize,
    is_rep: bool,
) -> Line<'static> {
    let filled = filled_cells(pct);
    let empty = BAR_WIDTH - filled;
    let bar_str = format!(
        "{}{}",
        symbols::ui::BAR_FULL.repeat(filled),
        symbols::ui::BAR_EMPTY.repeat(empty)
    );
    let color = theme.meter(pct);
    let label_style = if is_rep {
        theme.muted().add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        theme.muted()
    };
    let pct_str = format!("  {pct:>3.0}%  ");
    let label_w = 8usize;
    let left_w = 5 + label_w + BAR_WIDTH + UnicodeWidthStr::width(pct_str.as_str());
    let pad = area_width.saturating_sub(left_w + UnicodeWidthStr::width(right_text));
    Line::from(vec![
        Span::raw("     "),
        Span::styled(format!("{label:<label_w$}"), label_style),
        Span::styled(bar_str, color),
        Span::styled(pct_str, color),
        Span::styled(" ".repeat(pad), theme.muted()),
        Span::styled(right_text.to_owned(), theme.muted()),
    ])
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn staleness_label(cached_at: i64) -> Option<String> {
    #[allow(clippy::cast_sign_loss)]
    let age = (now_secs() - cached_at).max(0) as u64;
    if age < 60 {
        None
    } else if age < 3600 {
        Some(format!("{}m ago", age / 60))
    } else {
        Some(format!("{}h ago", age / 3600))
    }
}

fn format_reset(resets_at: Option<i64>) -> String {
    let Some(ts) = resets_at else {
        return String::new();
    };
    let remaining = ts - now_secs();
    if remaining <= 0 {
        return "expired".to_owned();
    }
    #[allow(clippy::cast_sign_loss)]
    let secs = remaining as u64;
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{h}h{m}m")
        } else {
            format!("{h}h")
        }
    } else {
        format!("{}d", secs / 86400)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn percent(used: u64, total: u32) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (used as f64 / f64::from(total) * 100.0).min(100.0) as f32
}

pub struct UsageView<'a> {
    account_entries: &'a [AccountEntry],
    usage_last: &'a HashMap<(String, String), Usage>,
    usage_total: &'a HashMap<(String, String), (u64, u64)>,
    rate_limits: &'a HashMap<(String, String), (RateLimitSnapshot, i64)>,
    context_window: Option<u32>,
    active_model: Option<&'a ModelTarget>,
    scroll: usize,
}

impl<'a> UsageView<'a> {
    pub fn new(
        account_entries: &'a [AccountEntry],
        usage_last: &'a HashMap<(String, String), Usage>,
        usage_total: &'a HashMap<(String, String), (u64, u64)>,
        rate_limits: &'a HashMap<(String, String), (RateLimitSnapshot, i64)>,
        context_window: Option<u32>,
        active_model: Option<&'a ModelTarget>,
        scroll: usize,
    ) -> Self {
        Self {
            account_entries,
            usage_last,
            usage_total,
            rate_limits,
            context_window,
            active_model,
            scroll,
        }
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.content_rows();
        clamp_u16(rows)
            .saturating_add(OVERLAY_CHROME_PLAIN)
            .clamp(8, 32)
    }

    fn content_rows(&self) -> usize {
        let mut rows = 0;
        for entry in self.account_entries {
            if entry.local {
                continue;
            }
            rows += 1;
            for account in &entry.accounts {
                rows += 1;
                let key = (entry.provider.clone(), account.name.clone());
                if matches!(account.method, AuthMethod::OAuth) {
                    if let Some((snapshot, _)) = self.rate_limits.get(&key) {
                        rows += snapshot.windows.len().max(1);
                    } else {
                        rows += 1;
                    }
                } else {
                    rows += 1;
                }
            }
        }
        if self.context_window.is_some() {
            rows += 3;
        }
        rows
    }

    #[allow(clippy::too_many_lines)]
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };

        let (body_area, hint_area) = overlay_layout_plain(inner);

        let w = usize::from(body_area.width);
        let mut lines: Vec<Line> = Vec::new();
        let mut first_provider = true;

        for entry in self.account_entries {
            if entry.local {
                continue;
            }
            if !first_provider {
                lines.push(Line::default());
            }
            first_provider = false;

            lines.push(Line::from(vec![Span::styled(
                format!(" {}", entry.provider),
                theme.accent(),
            )]));

            if entry.accounts.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled("no accounts", theme.muted()),
                ]));
                continue;
            }

            for account in &entry.accounts {
                let key = (entry.provider.clone(), account.name.clone());
                let is_active = self
                    .active_model
                    .is_some_and(|m| m.provider == entry.provider && m.account == account.name);
                let is_oauth = matches!(account.method, AuthMethod::OAuth);
                let auth_label = if is_oauth { "oauth" } else { "api key" };

                let (status_text, status_style) = if is_oauth {
                    match self.rate_limits.get(&key) {
                        Some((_, cached_at)) => match staleness_label(*cached_at) {
                            None => ("active".to_owned(), theme.success()),
                            Some(age) => (age, theme.muted()),
                        },
                        None => ("not used".to_owned(), theme.muted()),
                    }
                } else if self.usage_total.contains_key(&key) {
                    ("active".to_owned(), theme.success())
                } else {
                    ("not used".to_owned(), theme.muted())
                };

                let name_style = if is_active { theme.key() } else { theme.base() };
                let left_len =
                    3 + account.name.width() + symbols::ui::SEPARATOR.width() + auth_label.width();
                let status_len = status_text.width();
                let pad = w.saturating_sub(left_len + status_len + 2);
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(account.name.clone(), name_style),
                    Span::styled(symbols::ui::SEPARATOR, theme.muted()),
                    Span::styled(auth_label.to_owned(), theme.muted()),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(status_text, status_style),
                ]));

                if is_oauth {
                    match self.rate_limits.get(&key) {
                        Some((snapshot, _)) if !snapshot.windows.is_empty() => {
                            for window in &snapshot.windows {
                                let reset_str = format_reset(window.resets_at);
                                let is_rep = snapshot
                                    .representative
                                    .as_deref()
                                    .is_some_and(|r| r == window.label);
                                lines.push(render_bar_line(
                                    &window.label,
                                    window.used_percent,
                                    &reset_str,
                                    theme,
                                    w,
                                    is_rep,
                                ));
                            }
                        }
                        Some(_) => {
                            lines.push(Line::from(vec![
                                Span::raw("     "),
                                Span::styled("limits unavailable", theme.muted()),
                            ]));
                        }
                        None => {}
                    }
                } else if let Some(&(inp, out)) = self.usage_total.get(&key) {
                    let tokens_str = format!(
                        "{} {} {} out",
                        format_tokens(inp),
                        symbols::ui::MIDDOT,
                        format_tokens(out),
                    );
                    let session_label = "  this session";
                    let pad2 = w.saturating_sub(
                        5 + tokens_str.width() + UnicodeWidthStr::width(session_label),
                    );
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(tokens_str, theme.base()),
                        Span::raw(" ".repeat(pad2)),
                        Span::styled(session_label, theme.muted()),
                    ]));
                }
            }
        }

        if let (Some(window), Some(model)) = (self.context_window, self.active_model) {
            let key = (model.provider.clone(), model.account.clone());
            if let Some(usage) = self.usage_last.get(&key) {
                lines.push(Line::default());
                let this_thread = "  this thread";
                let ctx_label_pad = w.saturating_sub(
                    1 + UnicodeWidthStr::width("context") + UnicodeWidthStr::width(this_thread) + 3,
                );
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled("context", theme.accent()),
                    Span::raw(" ".repeat(ctx_label_pad)),
                    Span::styled(this_thread, theme.muted()),
                ]));
                let ctx_used = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
                let pct = percent(ctx_used, window);
                let detail = format!(
                    "{}/{}",
                    format_tokens(ctx_used),
                    format_tokens(u64::from(window))
                );
                lines.push(render_bar_line("", pct, &detail, theme, w, false));
            }
        }

        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(" no accounts {} /config to add", symbols::key::ARROW_RIGHT),
                theme.muted(),
            )));
        }

        let total = lines.len();
        let body_rows = usize::from(body_area.height);
        let win = overlay::window(self.scroll.min(total.saturating_sub(1)), total, body_rows);
        let mut visible: Vec<Line> = Vec::with_capacity(body_rows);
        if let Some(above) = &win.above {
            visible.push(Line::from(Span::styled(format!(" {above}"), theme.muted())));
        }
        for line in lines.into_iter().skip(win.start).take(win.shown) {
            visible.push(line);
        }
        if let Some(below) = &win.below {
            visible.push(Line::from(Span::styled(format!(" {below}"), theme.muted())));
        }
        frame.render_widget(Paragraph::new(visible), body_area);
        let _ = hint_area;
    }
}

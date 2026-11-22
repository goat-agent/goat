use crossterm::style::Stylize;
use goat_core::provider::Limits;

const SEGMENTS: usize = 10;
const WARN: u8 = 80;
const ALARM: u8 = 95;
const LABEL: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) label: String,
    percent: Option<u8>,
    detail: String,
}

impl Row {
    pub(crate) fn is_high(&self) -> bool {
        self.percent.is_some_and(|percent| percent >= WARN)
    }

    pub(crate) fn line(&self) -> String {
        let shown = match self.percent {
            Some(percent) => format!(
                "{} {}",
                bar(percent),
                paint(percent, &format!("{percent}%"))
            ),
            None => format!("{} {}", "▱".repeat(SEGMENTS).dim(), "-".dim()),
        };
        let detail = if self.detail.is_empty() {
            String::new()
        } else {
            format!("  {}", self.detail.as_str().dim())
        };
        format!(
            "{} {shown}{detail}",
            format!("{:<LABEL$}", self.label).dim()
        )
    }
}

pub(crate) fn rows(context: (u64, u64), limits: Option<&Limits>, now: u64) -> Vec<Row> {
    let (used, limit) = context;
    let mut rows = vec![Row {
        label: "Context".to_owned(),
        percent: (used > 0).then(|| share(used, limit)),
        detail: String::new(),
    }];
    for window in limits.map_or(&[][..], |limits| limits.windows.as_slice()) {
        rows.push(Row {
            label: window_label(window.minutes),
            percent: Some(window.used_percent),
            detail: window
                .resets_at
                .filter(|at| *at > now)
                .map_or_default(|at| format!("resets in {}", span(at - now))),
        });
    }
    rows
}

fn share(used: u64, limit: u64) -> u8 {
    u8::try_from((used.saturating_mul(100) / limit.max(1)).min(100)).unwrap_or(100)
}

fn bar(percent: u8) -> String {
    let filled = ((usize::from(percent) * SEGMENTS + 50) / 100).min(SEGMENTS);
    format!(
        "{}{}",
        paint(percent, &"▰".repeat(filled)),
        "▱".repeat(SEGMENTS - filled).dim()
    )
}

fn paint(percent: u8, text: &str) -> String {
    match percent {
        ALARM.. => text.red().to_string(),
        WARN.. => text.yellow().to_string(),
        _ => text.to_owned(),
    }
}

fn window_label(minutes: u64) -> String {
    match minutes {
        10_080 => "Weekly".to_owned(),
        1_440 => "Daily".to_owned(),
        minutes if minutes % 1_440 == 0 => format!("{}d", minutes / 1_440),
        minutes if minutes % 60 == 0 => format!("{}h", minutes / 60),
        minutes => format!("{minutes}m"),
    }
}

fn span(seconds: u64) -> String {
    let (days, hours, minutes) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{}m", minutes.max(1))
    }
}

#[cfg(test)]
mod tests {
    use goat_core::provider::Window;

    use super::*;

    fn limits(windows: &[(u64, u8, Option<u64>)]) -> Limits {
        Limits {
            plan: Some("pro".to_owned()),
            windows: windows
                .iter()
                .map(|&(minutes, used_percent, resets_at)| Window {
                    minutes,
                    used_percent,
                    resets_at,
                })
                .collect(),
        }
    }

    #[test]
    fn context_and_each_plan_window_get_a_bar() {
        let plan = limits(&[
            (300, 12, None),
            (10_080, 32, Some(1_000 + 3 * 86_400 + 7 * 3_600)),
        ]);

        let lines: Vec<String> = rows((48_000, 200_000), Some(&plan), 1_000)
            .iter()
            .map(Row::line)
            .collect();

        assert_eq!(
            lines[0],
            format!("{} {}{} 24%", "Context ".dim(), "▰▰", "▱▱▱▱▱▱▱▱".dim())
        );
        assert!(lines[1].starts_with(&format!("{}", "5h      ".dim())));
        assert!(lines[2].ends_with(&format!("{}", "resets in 3d 7h".dim())));
    }

    #[test]
    fn a_full_window_turns_red_and_counts_as_high() {
        let plan = limits(&[(10_080, 96, None)]);

        let rows = rows((0, 200_000), Some(&plan), 0);

        assert!(rows[1].line().contains(&"96%".red().to_string()));
        assert!(rows[1].is_high());
        assert!(!rows[0].is_high());
        assert!(rows[0].line().ends_with(&"-".dim().to_string()));
    }

    #[test]
    fn spans_and_labels_stay_short() {
        assert_eq!(span(5 * 3_600 + 12 * 60), "5h 12m");
        assert_eq!(span(30), "1m");
        assert_eq!(window_label(300), "5h");
        assert_eq!(window_label(10_080), "Weekly");
    }
}

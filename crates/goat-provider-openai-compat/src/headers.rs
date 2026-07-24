use goat_provider::{RateLimitSnapshot, RateWindow, now_secs};
use reqwest::header::HeaderMap;

pub fn parse_codex_ratelimits(headers: &HeaderMap) -> Option<RateLimitSnapshot> {
    let mut windows = Vec::new();

    if let Some(window) = parse_codex_window(headers, "primary", "5h") {
        windows.push(window);
    }
    if let Some(window) = parse_codex_window(headers, "secondary", "weekly") {
        windows.push(window);
    }

    if windows.is_empty() {
        None
    } else {
        Some(RateLimitSnapshot {
            windows,
            representative: None,
        })
    }
}

fn parse_codex_window(headers: &HeaderMap, prefix: &str, label: &str) -> Option<RateWindow> {
    let pct_key = format!("x-codex-{prefix}-used-percent");
    let reset_key = format!("x-codex-{prefix}-reset-after-seconds");

    let used_percent: f32 = headers
        .get(&pct_key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())?;

    let resets_at = headers
        .get(&reset_key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .map(|secs| now_secs() + secs);

    Some(RateWindow {
        label: label.to_owned(),
        used_percent,
        resets_at,
    })
}

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub(crate) fn summarize(v: &Value) -> String {
    let acct = v.get("account_id").and_then(|x| x.as_str()).unwrap_or("?");
    let acct_suffix = if acct.len() > 6 {
        &acct[acct.len() - 6..]
    } else {
        acct
    };
    let expires = v.get("expires_at_ms").and_then(|x| x.as_u64()).unwrap_or(0);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let label_suffix = v
        .get("label")
        .and_then(|x| x.as_str())
        .map(|l| format!("  {l}"))
        .unwrap_or_default();
    if expires <= now_ms {
        format!("ChatGPT acct …{acct_suffix}  EXPIRED{label_suffix}")
    } else {
        let secs_left = (expires - now_ms) / 1000;
        format!(
            "ChatGPT acct …{acct_suffix}  expires in {}{label_suffix}",
            human_duration(secs_left)
        )
    }
}

fn human_duration(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

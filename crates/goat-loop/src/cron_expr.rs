use std::str::FromStr;

use chrono::{DateTime, Local, Utc};
use cron::Schedule;

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("invalid cron expression: {0}")]
    Parse(String),
}

/// Parses a standard 5-field cron expression of the form
/// `minute hour day-of-month month day-of-week`.
///
/// Internally normalises to the `cron` crate's 7-field format
/// (seconds = 0, year = wildcard) so callers can keep using
/// the conventional 5-field grammar.
pub fn parse(five_field: &str) -> Result<Schedule, CronError> {
    let normalised = normalise_to_seven(five_field)?;
    Schedule::from_str(&normalised).map_err(|e| CronError::Parse(e.to_string()))
}

/// Returns the next occurrence strictly after `after`, evaluated in the
/// system's local timezone (the goat host's tz). The result is converted
/// back to UTC for storage.
pub fn next_after(schedule: &Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let local_after = after.with_timezone(&Local);
    schedule
        .after(&local_after)
        .next()
        .map(|d| d.with_timezone(&Utc))
}

/// Returns up to `n` upcoming occurrences strictly after `after`,
/// suitable for dry-run preview at task registration time.
pub fn upcoming(schedule: &Schedule, after: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
    let local_after = after.with_timezone(&Local);
    schedule
        .after(&local_after)
        .take(n)
        .map(|d| d.with_timezone(&Utc))
        .collect()
}

fn normalise_to_seven(five_field: &str) -> Result<String, CronError> {
    let trimmed = five_field.trim();
    let fields: Vec<&str> = trimmed.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(CronError::Parse(format!(
            "expected 5 fields, got {}: {trimmed:?}",
            fields.len()
        )));
    }
    let minute = fields[0];
    let hour = fields[1];
    let dom = fields[2];
    let month = fields[3];
    let dow = standard_dow_to_cron_crate(fields[4]);
    Ok(format!("0 {minute} {hour} {dom} {month} {dow} *"))
}

/// Translate a standard cron `day_of_week` field (0=Sun..6=Sat, also 7=Sun)
/// into the `cron` crate's `day_of_week` field (1=Sun..7=Sat).
///
/// Handles comma lists, hyphen ranges, and `base/step` increments. Tokens
/// that aren't pure digits (e.g. `*`, `MON`, `MON-FRI`) are passed through.
fn standard_dow_to_cron_crate(dow: &str) -> String {
    dow.split(',')
        .map(translate_dow_term)
        .collect::<Vec<_>>()
        .join(",")
}

fn translate_dow_term(term: &str) -> String {
    if let Some((base, step)) = term.split_once('/') {
        let base = translate_dow_token(base);
        return format!("{base}/{step}");
    }
    if let Some((a, b)) = term.split_once('-') {
        let a = translate_dow_token(a);
        let b = translate_dow_token(b);
        return format!("{a}-{b}");
    }
    translate_dow_token(term)
}

fn translate_dow_token(token: &str) -> String {
    if let Ok(n) = token.parse::<u32>() {
        let shifted = if n >= 7 { 1 } else { n + 1 };
        return shifted.to_string();
    }
    token.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

    #[test]
    fn parses_valid_weekly_pattern() {
        // Every Monday 07:00
        let schedule = parse("0 7 * * 1").expect("valid cron");
        let from = Utc.with_ymd_and_hms(2026, 5, 17, 0, 0, 0).unwrap(); // Sunday
        let next = next_after(&schedule, from).expect("has next");
        let local = next.with_timezone(&Local);
        assert_eq!(local.weekday(), chrono::Weekday::Mon);
        assert_eq!(local.hour(), 7);
        assert_eq!(local.minute(), 0);
    }

    #[test]
    fn parses_daily_pattern() {
        let schedule = parse("0 9 * * *").expect("valid cron");
        let from = Utc::now();
        let upcoming = upcoming(&schedule, from, 3);
        assert_eq!(upcoming.len(), 3);
        for occ in upcoming {
            let local = occ.with_timezone(&Local);
            assert_eq!(local.hour(), 9);
            assert_eq!(local.minute(), 0);
        }
    }

    #[test]
    fn upcoming_returns_strictly_after_anchor() {
        let schedule = parse("0 0 * * *").expect("valid cron");
        let anchor = Utc::now();
        let next = next_after(&schedule, anchor).unwrap();
        assert!(next > anchor);
    }

    #[test]
    fn rejects_short_expression() {
        assert!(parse("0 7 * *").is_err());
    }

    #[test]
    fn rejects_extra_fields() {
        assert!(parse("0 7 * * 1 2026").is_err());
    }

    #[test]
    fn rejects_invalid_minute_value() {
        let err = parse("99 * * * *").expect_err("99 is not a valid minute");
        assert!(matches!(err, CronError::Parse(_)));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("not a cron").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn sunday_is_zero_in_standard_form() {
        // Standard cron: 0 = Sunday. Verify our normaliser shifts it
        // correctly into the cron crate's 1 = Sunday encoding.
        let schedule = parse("0 9 * * 0").expect("valid sunday-only cron");
        let from = Utc.with_ymd_and_hms(2026, 5, 17, 0, 0, 0).unwrap(); // Sun
        for occ in upcoming(&schedule, from, 4) {
            let local = occ.with_timezone(&Local);
            assert_eq!(
                local.weekday(),
                chrono::Weekday::Sun,
                "every fired occurrence must be Sunday"
            );
            assert_eq!(local.hour(), 9);
        }
    }

    #[test]
    fn weekday_range_monday_to_friday() {
        let schedule = parse("0 9 * * 1-5").expect("valid weekday range");
        let from = Utc.with_ymd_and_hms(2026, 5, 17, 0, 0, 0).unwrap(); // Sun
        let mut weekdays = std::collections::HashSet::new();
        for occ in upcoming(&schedule, from, 10) {
            let local = occ.with_timezone(&Local);
            let wd = local.weekday();
            assert!(
                !matches!(wd, chrono::Weekday::Sat | chrono::Weekday::Sun),
                "weekend day {wd:?} should not be in 1-5 range"
            );
            weekdays.insert(wd);
        }
        assert!(weekdays.len() >= 3, "should see multiple weekdays");
    }

    #[test]
    fn weekday_list_sun_and_sat() {
        let schedule = parse("0 9 * * 0,6").expect("valid weekend list");
        let from = Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap(); // Mon
        for occ in upcoming(&schedule, from, 6) {
            let local = occ.with_timezone(&Local);
            let wd = local.weekday();
            assert!(
                matches!(wd, chrono::Weekday::Sat | chrono::Weekday::Sun),
                "only weekends expected, got {wd:?}"
            );
        }
    }
}

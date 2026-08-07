use std::str::FromStr;

use chrono::{DateTime, Local, Utc};
use chrono_tz::Tz;
use cron::Schedule;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CronTimezone {
    HostLocal,
    Named(Tz),
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("invalid cron expression: {0}")]
    Parse(String),
    #[error("invalid IANA timezone: {0}")]
    Timezone(String),
}

pub fn parse(five_field: &str) -> Result<Schedule, CronError> {
    let normalised = normalise_to_seven(five_field)?;
    Schedule::from_str(&normalised).map_err(|e| CronError::Parse(e.to_string()))
}

pub fn parse_timezone(name: Option<&str>) -> Result<CronTimezone, CronError> {
    match name {
        Some(name) => name
            .parse::<Tz>()
            .map(CronTimezone::Named)
            .map_err(|_| CronError::Timezone(name.to_string())),
        None => Ok(CronTimezone::HostLocal),
    }
}

pub fn next_after(
    schedule: &Schedule,
    after: DateTime<Utc>,
    timezone: CronTimezone,
) -> Option<DateTime<Utc>> {
    match timezone {
        CronTimezone::HostLocal => schedule
            .after(&after.with_timezone(&Local))
            .next()
            .map(|date| date.with_timezone(&Utc)),
        CronTimezone::Named(timezone) => schedule
            .after(&after.with_timezone(&timezone))
            .next()
            .map(|date| date.with_timezone(&Utc)),
    }
}

pub fn upcoming(
    schedule: &Schedule,
    after: DateTime<Utc>,
    n: usize,
    timezone: CronTimezone,
) -> Vec<DateTime<Utc>> {
    std::iter::successors(next_after(schedule, after, timezone), |previous| {
        next_after(schedule, *previous, timezone)
    })
    .take(n)
    .collect()
}

pub fn includes(schedule: &Schedule, occurrence: DateTime<Utc>, timezone: CronTimezone) -> bool {
    if occurrence.timestamp_subsec_nanos() != 0 {
        return false;
    }
    match timezone {
        CronTimezone::HostLocal => schedule.includes(occurrence.with_timezone(&Local)),
        CronTimezone::Named(timezone) => schedule.includes(occurrence.with_timezone(&timezone)),
    }
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

    fn host_local() -> CronTimezone {
        CronTimezone::HostLocal
    }

    fn seoul() -> CronTimezone {
        parse_timezone(Some("Asia/Seoul")).unwrap()
    }

    #[test]
    fn parses_valid_weekly_pattern() {
        let schedule = parse("0 7 * * 1").expect("valid cron");
        let from = Utc.with_ymd_and_hms(2026, 5, 17, 0, 0, 0).unwrap();
        let next = next_after(&schedule, from, host_local()).expect("has next");
        let local = next.with_timezone(&Local);
        assert_eq!(local.weekday(), chrono::Weekday::Mon);
        assert_eq!(local.hour(), 7);
        assert_eq!(local.minute(), 0);
    }

    #[test]
    fn parses_daily_pattern() {
        let schedule = parse("0 9 * * *").expect("valid cron");
        let from = Utc::now();
        let upcoming = upcoming(&schedule, from, 3, host_local());
        assert_eq!(upcoming.len(), 3);
        for occurrence in upcoming {
            let local = occurrence.with_timezone(&Local);
            assert_eq!(local.hour(), 9);
            assert_eq!(local.minute(), 0);
        }
    }

    #[test]
    fn named_timezone_is_independent_of_host_timezone() {
        let schedule = parse("0 9 * * *").unwrap();
        let from = Utc.with_ymd_and_hms(2026, 1, 1, 0, 1, 0).unwrap();
        let next = next_after(&schedule, from, seoul()).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap());
    }

    #[test]
    fn daily_schedule_skips_nonexistent_dst_wall_time() {
        let schedule = parse("30 2 * * *").unwrap();
        let timezone = parse_timezone(Some("America/New_York")).unwrap();
        let from = Utc.with_ymd_and_hms(2026, 3, 7, 0, 0, 0).unwrap();
        let occurrences = upcoming(&schedule, from, 3, timezone);
        assert_eq!(
            occurrences,
            vec![
                Utc.with_ymd_and_hms(2026, 3, 7, 7, 30, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 3, 9, 6, 30, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 3, 10, 6, 30, 0).unwrap(),
            ]
        );
    }

    #[test]
    fn daily_schedule_fires_once_across_repeated_dst_wall_time() {
        let schedule = parse("30 1 * * *").unwrap();
        let timezone = parse_timezone(Some("America/New_York")).unwrap();
        let from = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        let occurrences = upcoming(&schedule, from, 2, timezone);
        assert_eq!(
            occurrences,
            vec![
                Utc.with_ymd_and_hms(2026, 11, 1, 5, 30, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 11, 2, 6, 30, 0).unwrap(),
            ]
        );
    }

    #[test]
    fn occurrence_must_match_cron_in_its_timezone() {
        let schedule = parse("0 9 * * *").unwrap();
        let matching = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
        let disagreeing = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        assert!(includes(&schedule, matching, seoul()));
        assert!(!includes(&schedule, disagreeing, seoul()));
    }

    #[test]
    fn upcoming_returns_strictly_after_anchor() {
        let schedule = parse("0 0 * * *").expect("valid cron");
        let anchor = Utc::now();
        let next = next_after(&schedule, anchor, host_local()).unwrap();
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
    fn rejects_invalid_timezone() {
        assert!(parse_timezone(Some("Mars/Olympus")).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("not a cron").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn sunday_is_zero_in_standard_form() {
        let schedule = parse("0 9 * * 0").expect("valid sunday-only cron");
        let from = Utc.with_ymd_and_hms(2026, 5, 17, 0, 0, 0).unwrap();
        for occurrence in upcoming(&schedule, from, 4, host_local()) {
            let local = occurrence.with_timezone(&Local);
            assert_eq!(local.weekday(), chrono::Weekday::Sun);
            assert_eq!(local.hour(), 9);
        }
    }

    #[test]
    fn weekday_range_monday_to_friday() {
        let schedule = parse("0 9 * * 1-5").expect("valid weekday range");
        let from = Utc.with_ymd_and_hms(2026, 5, 17, 0, 0, 0).unwrap();
        let mut weekdays = std::collections::HashSet::new();
        for occurrence in upcoming(&schedule, from, 10, host_local()) {
            let weekday = occurrence.with_timezone(&Local).weekday();
            assert!(!matches!(
                weekday,
                chrono::Weekday::Sat | chrono::Weekday::Sun
            ));
            weekdays.insert(weekday);
        }
        assert!(weekdays.len() >= 3);
    }

    #[test]
    fn weekday_list_sun_and_sat() {
        let schedule = parse("0 9 * * 0,6").expect("valid weekend list");
        let from = Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap();
        for occurrence in upcoming(&schedule, from, 6, host_local()) {
            let weekday = occurrence.with_timezone(&Local).weekday();
            assert!(matches!(
                weekday,
                chrono::Weekday::Sat | chrono::Weekday::Sun
            ));
        }
    }
}

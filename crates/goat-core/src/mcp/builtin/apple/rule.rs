use std::fmt;

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Weekday};
use jiff::tz::TimeZone;

const PREFIX: &str = "RRULE:";
const DAY_FORMAT: &str = "%Y%m%d";
const TIME_FORMAT: &str = "%Y%m%dT%H%M%S";
const WEEKDAYS: [(Weekday, &str); 7] = [
    (Weekday::Monday, "MO"),
    (Weekday::Tuesday, "TU"),
    (Weekday::Wednesday, "WE"),
    (Weekday::Thursday, "TH"),
    (Weekday::Friday, "FR"),
    (Weekday::Saturday, "SA"),
    (Weekday::Sunday, "SU"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Day {
    pub(crate) weekday: Weekday,
    pub(crate) week: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Until {
    Day(Date),
    Floating(DateTime),
    At(Timestamp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    Count(u32),
    Until(Until),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) frequency: Frequency,
    pub(crate) interval: u32,
    pub(crate) days: Vec<Day>,
    pub(crate) month_days: Vec<i8>,
    pub(crate) months: Vec<i8>,
    pub(crate) positions: Vec<i16>,
    pub(crate) end: Option<End>,
}

impl Rule {
    pub(crate) fn new(frequency: Frequency) -> Self {
        Self {
            frequency,
            interval: 1,
            days: Vec::new(),
            month_days: Vec::new(),
            months: Vec::new(),
            positions: Vec::new(),
            end: None,
        }
    }

    pub(crate) fn parse(line: &str) -> Result<Self, String> {
        let line = line.trim();
        if line.starts_with("RDATE") || line.starts_with("EXDATE") {
            return Err(format!(
                "{line} is not supported; Apple takes RRULE lines only"
            ));
        }
        let body = line.strip_prefix(PREFIX).unwrap_or(line);
        let mut frequency = None;
        let mut rule = Self::new(Frequency::Daily);
        for part in body.split(';').filter(|part| !part.is_empty()) {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| format!("{part} in the rule has no ="))?;
            let wrong = || format!("{key}={value} is not a valid rule part");
            match key.to_ascii_uppercase().as_str() {
                "FREQ" => frequency = Some(parse_frequency(value)?),
                "INTERVAL" => {
                    rule.interval = value.parse().ok().filter(|n| *n > 0).ok_or_else(wrong)?;
                }
                "COUNT" => {
                    let count = value.parse().ok().filter(|n| *n > 0).ok_or_else(wrong)?;
                    rule.end = Some(End::Count(count));
                }
                "UNTIL" => rule.end = Some(End::Until(parse_until(value).ok_or_else(wrong)?)),
                "BYDAY" => rule.days = list(value, parse_day).ok_or_else(wrong)?,
                "BYMONTHDAY" => {
                    rule.month_days = list(value, |n| {
                        n.parse()
                            .ok()
                            .filter(|n: &i8| (1..=31).contains(&n.unsigned_abs()))
                    })
                    .ok_or_else(wrong)?;
                }
                "BYMONTH" => {
                    rule.months = list(value, |n| n.parse().ok().filter(|n| (1..=12).contains(n)))
                        .ok_or_else(wrong)?;
                }
                "BYSETPOS" => {
                    rule.positions = list(value, |n| {
                        n.parse()
                            .ok()
                            .filter(|n: &i16| (1..=366).contains(&n.unsigned_abs()))
                    })
                    .ok_or_else(wrong)?;
                }
                "WKST" => {}
                _ => return Err(format!("{key} is not supported in a rule")),
            }
        }
        rule.frequency = frequency.ok_or("the rule needs FREQ")?;
        Ok(rule)
    }

    pub(crate) fn to_line(&self) -> String {
        format!("{PREFIX}{self}")
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let frequency = match self.frequency {
            Frequency::Daily => "DAILY",
            Frequency::Weekly => "WEEKLY",
            Frequency::Monthly => "MONTHLY",
            Frequency::Yearly => "YEARLY",
        };
        write!(f, "FREQ={frequency}")?;
        if self.interval != 1 {
            write!(f, ";INTERVAL={}", self.interval)?;
        }
        if !self.days.is_empty() {
            let days: Vec<String> = self.days.iter().map(Day::to_string).collect();
            write!(f, ";BYDAY={}", days.join(","))?;
        }
        write_list(f, "BYMONTHDAY", &self.month_days)?;
        write_list(f, "BYMONTH", &self.months)?;
        write_list(f, "BYSETPOS", &self.positions)?;
        match self.end {
            None => Ok(()),
            Some(End::Count(count)) => write!(f, ";COUNT={count}"),
            Some(End::Until(Until::Day(date))) => write!(f, ";UNTIL={}", date.strftime(DAY_FORMAT)),
            Some(End::Until(Until::Floating(time))) => {
                write!(f, ";UNTIL={}", time.strftime(TIME_FORMAT))
            }
            Some(End::Until(Until::At(at))) => {
                write!(f, ";UNTIL={}Z", at.strftime(TIME_FORMAT))
            }
        }
    }
}

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = WEEKDAYS
            .iter()
            .find(|(weekday, _)| *weekday == self.weekday)
            .map_or("MO", |(_, code)| code);
        if self.week == 0 {
            write!(f, "{code}")
        } else {
            write!(f, "{}{code}", self.week)
        }
    }
}

fn parse_frequency(value: &str) -> Result<Frequency, String> {
    match value.to_ascii_uppercase().as_str() {
        "DAILY" => Ok(Frequency::Daily),
        "WEEKLY" => Ok(Frequency::Weekly),
        "MONTHLY" => Ok(Frequency::Monthly),
        "YEARLY" => Ok(Frequency::Yearly),
        _ => Err(format!(
            "FREQ={value} is not supported; use DAILY, WEEKLY, MONTHLY or YEARLY"
        )),
    }
}

fn list<T>(value: &str, parse: impl Fn(&str) -> Option<T>) -> Option<Vec<T>> {
    value.split(',').map(|item| parse(item.trim())).collect()
}

fn parse_day(text: &str) -> Option<Day> {
    let split = text.len().checked_sub(2)?;
    let (week, code) = text.split_at_checked(split)?;
    let weekday = WEEKDAYS
        .iter()
        .find(|(_, known)| known.eq_ignore_ascii_case(code))?
        .0;
    let week = if week.is_empty() {
        0
    } else {
        week.parse()
            .ok()
            .filter(|n: &i8| (1..=53).contains(&n.unsigned_abs()))?
    };
    Some(Day { weekday, week })
}

fn parse_until(text: &str) -> Option<Until> {
    if let Some(utc) = text.strip_suffix(['Z', 'z']) {
        let time = DateTime::strptime(TIME_FORMAT, utc).ok()?;
        return time
            .to_zoned_datetime(TimeZone::UTC)
            .ok()
            .map(|at| Until::At(at.timestamp()));
    }
    if text.contains(['T', 't']) {
        return DateTime::strptime(TIME_FORMAT, text)
            .ok()
            .map(Until::Floating);
    }
    Date::strptime(DAY_FORMAT, text).ok().map(Until::Day)
}

fn write_list<T: fmt::Display>(f: &mut fmt::Formatter<'_>, key: &str, items: &[T]) -> fmt::Result {
    if items.is_empty() {
        return Ok(());
    }
    let items: Vec<String> = items.iter().map(T::to_string).collect();
    write!(f, ";{key}={}", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weekly_rule_with_days_and_a_count_round_trips() {
        let rule = Rule::parse("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10").unwrap();

        assert_eq!(rule.frequency, Frequency::Weekly);
        assert_eq!(rule.interval, 2);
        assert_eq!(rule.end, Some(End::Count(10)));
        assert_eq!(
            rule.to_line(),
            "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10"
        );
    }

    #[test]
    fn an_ordinal_weekday_and_each_kind_of_until_are_read() {
        let utc = Rule::parse("FREQ=MONTHLY;BYDAY=-1FR;UNTIL=20271231T150000Z").unwrap();
        let floating = Rule::parse("FREQ=DAILY;UNTIL=20271231T090000").unwrap();
        let day = Rule::parse("FREQ=DAILY;UNTIL=20271231").unwrap();

        assert_eq!(
            utc.days,
            [Day {
                weekday: Weekday::Friday,
                week: -1
            }]
        );
        assert_eq!(
            utc.to_string(),
            "FREQ=MONTHLY;BYDAY=-1FR;UNTIL=20271231T150000Z"
        );
        assert_eq!(floating.to_string(), "FREQ=DAILY;UNTIL=20271231T090000");
        assert_eq!(day.to_string(), "FREQ=DAILY;UNTIL=20271231");
    }

    #[test]
    fn lines_apple_cannot_store_are_rejected() {
        assert_eq!(Rule::parse("COUNT=3").unwrap_err(), "the rule needs FREQ");
        assert_eq!(
            Rule::parse("FREQ=HOURLY").unwrap_err(),
            "FREQ=HOURLY is not supported; use DAILY, WEEKLY, MONTHLY or YEARLY"
        );
        assert_eq!(
            Rule::parse("FREQ=DAILY;BYHOUR=9").unwrap_err(),
            "BYHOUR is not supported in a rule"
        );
        assert!(
            Rule::parse("EXDATE:20261126T090000Z")
                .unwrap_err()
                .contains("RRULE lines only")
        );
    }
}

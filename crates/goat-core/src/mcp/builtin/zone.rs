use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use serde_json::{Value, json};

pub(super) struct Zone {
    pub(super) name: String,
    pub(super) tz: TimeZone,
}

impl Zone {
    pub(super) fn named(name: String) -> Result<Self, String> {
        let tz = TimeZone::get(&name)
            .map_err(|err| format!("{name} is not a known time zone: {err}"))?;
        Ok(Self { name, tz })
    }

    pub(super) fn at(&self, text: &str) -> Result<ZonedDateTime, String> {
        if let Ok(instant) = text.parse::<Timestamp>() {
            return Ok(instant.to_zoned_datetime(self.tz.clone()));
        }
        if let Ok(civil) = text.parse::<DateTime>() {
            return self.civil(civil);
        }
        if let Ok(day) = text.parse::<Date>() {
            return self.civil(day.to_datetime(Time::midnight()));
        }
        Err(format!("{text} is not an ISO 8601 time"))
    }

    pub(super) fn civil(&self, civil: DateTime) -> Result<ZonedDateTime, String> {
        self.tz
            .to_zoned_datetime(civil)
            .map_err(|err| format!("{civil} does not exist in {}: {err}", self.name))
    }

    pub(super) fn moment(&self, at: &ZonedDateTime) -> Value {
        json!({ "dateTime": at.datetime().to_string(), "timeZone": self.name })
    }

    pub(super) fn instant(&self, at: Timestamp) -> Value {
        self.moment(&at.to_zoned_datetime(self.tz.clone()))
    }

    pub(super) fn days(&self, start: &str, end: &str) -> Result<(Date, Date), String> {
        let first = self.at(start)?.date();
        let mut last = self.at(end)?.date();
        if last <= first {
            last = first.tomorrow().map_err(|err| err.to_string())?;
        }
        Ok((first, last))
    }

    pub(super) fn span(
        &self,
        start: &str,
        end: &str,
        is_all_day: bool,
    ) -> Result<(Value, Value), String> {
        if !is_all_day {
            return Ok((self.moment(&self.at(start)?), self.moment(&self.at(end)?)));
        }
        let (first, last) = self.days(start, end)?;
        Ok((
            json!({ "date": first.to_string() }),
            json!({ "date": last.to_string() }),
        ))
    }

    pub(super) fn length(&self, found: &Value) -> Result<i64, String> {
        let instant = |moment: &Value| -> Option<Timestamp> {
            moment["dateTime"]
                .as_str()
                .or_else(|| moment["date"].as_str())
                .and_then(|text| self.at(text).ok())
                .map(|at| at.timestamp())
        };
        let (Some(start), Some(end)) = (instant(&found["start"]), instant(&found["end"])) else {
            return Err("the event has no start and end to keep its length from".to_owned());
        };
        Ok(end.as_second() - start.as_second())
    }

    pub(super) fn slot(&self, start: Timestamp, end: Timestamp) -> Value {
        json!({ "start": self.instant(start), "end": self.instant(end) })
    }
}

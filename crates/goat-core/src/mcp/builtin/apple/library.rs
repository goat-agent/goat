use jiff::Timestamp;
use jiff::civil::Date;

use super::super::worker::{self, Worker};
use super::rule::Rule;

pub(crate) type Shelf = Worker<dyn Library>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Area {
    Calendars,
    Reminders,
    Contacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Events,
    Reminders,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Moment {
    Day(Date),
    At(Timestamp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(target_os = "macos"),
    expect(dead_code, reason = "only the macOS adapter reads attendees so far")
)]
pub(crate) enum Response {
    NeedsAction,
    Accepted,
    Declined,
    Tentative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Priority {
    #[default]
    None,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Open,
    Done,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum Field<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Color {
    pub(crate) red: u8,
    pub(crate) green: u8,
    pub(crate) blue: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Calendar {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) kind: Kind,
    pub(crate) account: String,
    pub(crate) color: Option<Color>,
    pub(crate) is_writable: bool,
    pub(crate) is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CalendarFields {
    pub(crate) title: Option<String>,
    pub(crate) kind: Option<Kind>,
    pub(crate) account: Option<String>,
    pub(crate) color: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Person {
    pub(crate) name: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) response: Response,
    pub(crate) is_self: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) id: String,
    pub(crate) series: Option<String>,
    pub(crate) calendar: String,
    pub(crate) title: String,
    pub(crate) start: Moment,
    pub(crate) end: Moment,
    pub(crate) location: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) alerts: Vec<i64>,
    pub(crate) rules: Vec<Rule>,
    pub(crate) organizer: Option<Person>,
    pub(crate) attendees: Vec<Person>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EventFields {
    pub(crate) calendar: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) start: Option<Moment>,
    pub(crate) end: Option<Moment>,
    pub(crate) location: Field<String>,
    pub(crate) notes: Field<String>,
    pub(crate) url: Field<String>,
    pub(crate) alerts: Option<Vec<i64>>,
    pub(crate) rules: Option<Vec<Rule>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Page {
    pub(crate) skip: usize,
    pub(crate) limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventQuery {
    pub(crate) from: Timestamp,
    pub(crate) to: Timestamp,
    pub(crate) calendars: Vec<String>,
    pub(crate) text: Option<String>,
    pub(crate) page: Page,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reminder {
    pub(crate) id: String,
    pub(crate) list: String,
    pub(crate) title: String,
    pub(crate) notes: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) due: Option<Moment>,
    pub(crate) priority: Priority,
    pub(crate) is_done: bool,
    pub(crate) completed: Option<Timestamp>,
    pub(crate) alerts: Vec<i64>,
    pub(crate) rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ReminderFields {
    pub(crate) list: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) notes: Field<String>,
    pub(crate) url: Field<String>,
    pub(crate) due: Field<Moment>,
    pub(crate) priority: Option<Priority>,
    pub(crate) is_done: Option<bool>,
    pub(crate) alerts: Option<Vec<i64>>,
    pub(crate) rules: Option<Vec<Rule>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReminderQuery {
    pub(crate) lists: Vec<String>,
    pub(crate) status: Status,
    pub(crate) from: Option<Timestamp>,
    pub(crate) to: Option<Timestamp>,
    pub(crate) text: Option<String>,
    pub(crate) page: Page,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Labeled {
    pub(crate) label: Option<String>,
    pub(crate) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Address {
    pub(crate) label: Option<String>,
    pub(crate) street: String,
    pub(crate) city: String,
    pub(crate) state: String,
    pub(crate) postal_code: String,
    pub(crate) country: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Birthday {
    pub(crate) year: Option<i16>,
    pub(crate) month: i8,
    pub(crate) day: i8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Contact {
    pub(crate) id: String,
    pub(crate) given_name: String,
    pub(crate) middle_name: String,
    pub(crate) family_name: String,
    pub(crate) nickname: String,
    pub(crate) organization: String,
    pub(crate) department: String,
    pub(crate) job_title: String,
    pub(crate) emails: Vec<Labeled>,
    pub(crate) phones: Vec<Labeled>,
    pub(crate) addresses: Vec<Address>,
    pub(crate) urls: Vec<Labeled>,
    pub(crate) birthday: Option<Birthday>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ContactFields {
    pub(crate) given_name: Option<String>,
    pub(crate) middle_name: Option<String>,
    pub(crate) family_name: Option<String>,
    pub(crate) nickname: Option<String>,
    pub(crate) organization: Option<String>,
    pub(crate) department: Option<String>,
    pub(crate) job_title: Option<String>,
    pub(crate) emails: Option<Vec<Labeled>>,
    pub(crate) phones: Option<Vec<Labeled>>,
    pub(crate) addresses: Option<Vec<Address>>,
    pub(crate) urls: Option<Vec<Labeled>>,
    pub(crate) birthday: Field<Birthday>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listing<T> {
    pub(crate) items: Vec<T>,
    pub(crate) is_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "only the macOS adapter looks items up so far")
    )]
    Missing(String),
    Denied(String),
    Unsupported(String),
    Failed(String),
}

pub(crate) trait Library {
    fn granted(&mut self) -> Vec<Area>;
    fn time_zone(&mut self) -> String;
    fn calendars(&mut self) -> Result<Vec<Calendar>, Fault>;
    fn create_calendar(&mut self, fields: CalendarFields) -> Result<Calendar, Fault>;
    fn update_calendar(&mut self, id: &str, fields: CalendarFields) -> Result<Calendar, Fault>;
    fn delete_calendar(&mut self, id: &str) -> Result<Calendar, Fault>;
    fn events(&mut self, query: &EventQuery) -> Result<Listing<Event>, Fault>;
    fn event(&mut self, id: &str) -> Result<Event, Fault>;
    fn create_event(&mut self, fields: EventFields) -> Result<Event, Fault>;
    fn update_event(&mut self, id: &str, fields: EventFields) -> Result<Event, Fault>;
    fn delete_event(&mut self, id: &str) -> Result<Event, Fault>;
    fn reminders(&mut self, query: &ReminderQuery) -> Result<Listing<Reminder>, Fault>;
    fn create_reminder(&mut self, fields: ReminderFields) -> Result<Reminder, Fault>;
    fn update_reminder(&mut self, id: &str, fields: ReminderFields) -> Result<Reminder, Fault>;
    fn delete_reminder(&mut self, id: &str) -> Result<Reminder, Fault>;
    fn contacts(&mut self, query: Option<&str>, page: Page) -> Result<Listing<Contact>, Fault>;
    fn contact(&mut self, id: &str) -> Result<Contact, Fault>;
    fn me(&mut self) -> Result<Contact, Fault>;
    fn create_contact(&mut self, fields: ContactFields) -> Result<Contact, Fault>;
    fn update_contact(&mut self, id: &str, fields: ContactFields) -> Result<Contact, Fault>;
    fn delete_contact(&mut self, id: &str) -> Result<Contact, Fault>;
}

impl Area {
    pub(crate) fn settings_name(self) -> &'static str {
        match self {
            Self::Calendars => "Calendars",
            Self::Reminders => "Reminders",
            Self::Contacts => "Contacts",
        }
    }
}

impl Kind {
    pub(crate) fn area(self) -> Area {
        match self {
            Self::Events => Area::Calendars,
            Self::Reminders => Area::Reminders,
        }
    }
}

impl Color {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        Some(Self {
            red: channel(0)?,
            green: channel(2)?,
            blue: channel(4)?,
        })
    }

    pub(crate) fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }
}

impl Birthday {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (year, rest) = if let Some(rest) = text.strip_prefix("--") {
            (None, rest)
        } else {
            let (year, rest) = text.split_once('-')?;
            (Some(year.parse().ok()?), rest)
        };
        let (month, day) = rest.split_once('-')?;
        let birthday = Self {
            year,
            month: month.parse().ok()?,
            day: day.parse().ok()?,
        };
        Date::new(year.unwrap_or(2000), birthday.month, birthday.day).ok()?;
        Some(birthday)
    }

    pub(crate) fn to_text(self) -> String {
        match self.year {
            Some(year) => format!("{year:04}-{:02}-{:02}", self.month, self.day),
            None => format!("--{:02}-{:02}", self.month, self.day),
        }
    }
}

impl<T> Listing<T> {
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "only the macOS adapter lists items so far")
    )]
    pub(crate) fn gather(found: impl IntoIterator<Item = T>, page: Page) -> Self {
        let mut found = found.into_iter().skip(page.skip);
        let items: Vec<T> = found.by_ref().take(page.limit).collect();
        Self {
            items,
            is_truncated: found.next().is_some(),
        }
    }
}

impl Fault {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Missing(_) => "not_found",
            Self::Denied(_) => "denied",
            Self::Unsupported(_) => "unsupported",
            Self::Failed(_) => "failed",
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Missing(text)
            | Self::Denied(text)
            | Self::Unsupported(text)
            | Self::Failed(text) => text,
        }
    }
}

impl From<worker::Error> for Fault {
    fn from(err: worker::Error) -> Self {
        Self::Failed(err.to_string())
    }
}

#[cfg_attr(
    all(not(target_os = "macos"), not(test)),
    expect(dead_code, reason = "only the macOS adapter searches so far")
)]
pub(crate) fn mentions(query: Option<&str>, texts: &[Option<&str>]) -> bool {
    let Some(query) = query else {
        return true;
    };
    let query = query.to_lowercase();
    texts
        .iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(&query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_skips_then_takes_and_says_whether_more_remain() {
        let page = Listing::gather(1..=5, Page { skip: 1, limit: 3 });

        assert_eq!(page.items, [2, 3, 4]);
        assert!(page.is_truncated);
        assert!(!Listing::gather(1..=3, Page { skip: 0, limit: 3 }).is_truncated);
    }

    #[test]
    fn colors_are_hex_and_birthdays_may_omit_the_year() {
        assert_eq!(Color::parse("#1a2B3c").unwrap().to_hex(), "#1A2B3C");
        assert_eq!(Color::parse("1a2b3c"), None);
        assert_eq!(Birthday::parse("--02-29").unwrap().to_text(), "--02-29");
        assert_eq!(
            Birthday::parse("1990-05-01").unwrap().to_text(),
            "1990-05-01"
        );
        assert_eq!(Birthday::parse("1990-02-30"), None);
    }
}

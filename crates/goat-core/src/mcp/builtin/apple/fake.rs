use std::sync::{Arc, Mutex};

use jiff::civil::Time;
use jiff::tz::TimeZone;

use super::library::{
    Area, Calendar, CalendarFields, Contact, ContactFields, Event, EventFields, EventQuery, Fault,
    Field, Kind, Library, Listing, Moment, Page, Priority, Reminder, ReminderFields, ReminderQuery,
    Status, mentions,
};

pub(crate) struct Fake {
    granted: Vec<Area>,
    calendars: Vec<Calendar>,
    events: Vec<Event>,
    reminders: Vec<Reminder>,
    contacts: Vec<Contact>,
    next: usize,
    pub(crate) log: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    pub(crate) fn new(granted: &[Area]) -> Self {
        let calendar = |id: &str, title: &str, kind, is_default| Calendar {
            id: id.to_owned(),
            title: title.to_owned(),
            kind,
            account: "iCloud".to_owned(),
            color: None,
            is_writable: true,
            is_default,
        };
        Self {
            granted: granted.to_vec(),
            calendars: vec![
                calendar("c1", "Home", Kind::Events, true),
                calendar("c2", "Work", Kind::Events, false),
                calendar("l1", "Reminders", Kind::Reminders, true),
            ],
            events: Vec::new(),
            reminders: Vec::new(),
            contacts: Vec::new(),
            next: 0,
            log: Arc::default(),
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn note(&self, line: String) {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }

    fn calendar_for(&self, id: Option<&str>, kind: Kind) -> Result<String, Fault> {
        self.calendars
            .iter()
            .find(|calendar| {
                calendar.kind == kind && id.map_or(calendar.is_default, |id| calendar.id == id)
            })
            .map(|calendar| calendar.id.clone())
            .ok_or_else(|| Fault::Missing(format!("no calendar {}", id.unwrap_or("default"))))
    }

    fn take<T>(items: &mut Vec<T>, found: impl Fn(&T) -> bool, what: &str) -> Result<T, Fault> {
        let at = items
            .iter()
            .position(found)
            .ok_or_else(|| Fault::Missing(format!("no {what}")))?;
        Ok(items.remove(at))
    }
}

impl Library for Fake {
    fn granted(&mut self) -> Vec<Area> {
        self.granted.clone()
    }

    fn time_zone(&mut self) -> String {
        "Asia/Seoul".to_owned()
    }

    fn calendars(&mut self) -> Result<Vec<Calendar>, Fault> {
        Ok(self.calendars.clone())
    }

    fn create_calendar(&mut self, fields: CalendarFields) -> Result<Calendar, Fault> {
        let calendar = Calendar {
            id: self.id("c"),
            title: fields.title.unwrap_or_default(),
            kind: fields.kind.unwrap_or(Kind::Events),
            account: fields.account.unwrap_or_else(|| "iCloud".to_owned()),
            color: fields.color,
            is_writable: true,
            is_default: false,
        };
        self.calendars.push(calendar.clone());
        Ok(calendar)
    }

    fn update_calendar(&mut self, id: &str, fields: CalendarFields) -> Result<Calendar, Fault> {
        let calendar = self
            .calendars
            .iter_mut()
            .find(|calendar| calendar.id == id)
            .ok_or_else(|| Fault::Missing(format!("no calendar {id}")))?;
        if let Some(title) = fields.title {
            calendar.title = title;
        }
        if let Some(color) = fields.color {
            calendar.color = Some(color);
        }
        Ok(calendar.clone())
    }

    fn delete_calendar(&mut self, id: &str) -> Result<Calendar, Fault> {
        self.note(format!("delete_calendar {id}"));
        Self::take(
            &mut self.calendars,
            |calendar| calendar.id == id,
            "calendar",
        )
    }

    fn events(&mut self, query: &EventQuery) -> Result<Listing<Event>, Fault> {
        let (from, to) = (query.from.as_second(), query.to.as_second());
        let mut found: Vec<Event> = self
            .events
            .iter()
            .filter(|event| query.calendars.is_empty() || query.calendars.contains(&event.calendar))
            .filter(|event| seconds(event.end) >= from && seconds(event.start) <= to)
            .filter(|event| {
                mentions(
                    query.text.as_deref(),
                    &[
                        Some(&event.title),
                        event.location.as_deref(),
                        event.notes.as_deref(),
                    ],
                )
            })
            .cloned()
            .collect();
        found.sort_by_key(|event| seconds(event.start));
        Ok(Listing::gather(found, query.page))
    }

    fn event(&mut self, id: &str) -> Result<Event, Fault> {
        self.events
            .iter()
            .find(|event| event.id == id)
            .cloned()
            .ok_or_else(|| Fault::Missing(format!("no event {id}")))
    }

    fn create_event(&mut self, fields: EventFields) -> Result<Event, Fault> {
        let calendar = self.calendar_for(fields.calendar.as_deref(), Kind::Events)?;
        let start = fields
            .start
            .ok_or_else(|| Fault::Failed("no start".to_owned()))?;
        let mut event = Event {
            id: self.id("e"),
            series: None,
            calendar,
            title: String::new(),
            start,
            end: start,
            location: None,
            notes: None,
            url: None,
            alerts: Vec::new(),
            rules: Vec::new(),
            organizer: None,
            attendees: Vec::new(),
        };
        apply_event(&mut event, fields);
        self.events.push(event.clone());
        Ok(event)
    }

    fn update_event(&mut self, id: &str, fields: EventFields) -> Result<Event, Fault> {
        let event = self
            .events
            .iter_mut()
            .find(|event| event.id == id)
            .ok_or_else(|| Fault::Missing(format!("no event {id}")))?;
        apply_event(event, fields);
        Ok(event.clone())
    }

    fn delete_event(&mut self, id: &str) -> Result<Event, Fault> {
        self.note(format!("delete_event {id}"));
        Self::take(&mut self.events, |event| event.id == id, "event")
    }

    fn reminders(&mut self, query: &ReminderQuery) -> Result<Listing<Reminder>, Fault> {
        let found = self
            .reminders
            .iter()
            .filter(|reminder| query.lists.is_empty() || query.lists.contains(&reminder.list))
            .filter(|reminder| match query.status {
                Status::Open => !reminder.is_done,
                Status::Done => reminder.is_done,
                Status::All => true,
            })
            .filter(|reminder| {
                mentions(
                    query.text.as_deref(),
                    &[Some(&reminder.title), reminder.notes.as_deref()],
                )
            })
            .cloned();
        Ok(Listing::gather(found, query.page))
    }

    fn create_reminder(&mut self, fields: ReminderFields) -> Result<Reminder, Fault> {
        let list = self.calendar_for(fields.list.as_deref(), Kind::Reminders)?;
        let mut reminder = Reminder {
            id: self.id("r"),
            list,
            title: String::new(),
            notes: None,
            url: None,
            due: None,
            priority: Priority::None,
            is_done: false,
            completed: None,
            alerts: Vec::new(),
            rules: Vec::new(),
        };
        apply_reminder(&mut reminder, fields);
        self.reminders.push(reminder.clone());
        Ok(reminder)
    }

    fn update_reminder(&mut self, id: &str, fields: ReminderFields) -> Result<Reminder, Fault> {
        let reminder = self
            .reminders
            .iter_mut()
            .find(|reminder| reminder.id == id)
            .ok_or_else(|| Fault::Missing(format!("no reminder {id}")))?;
        apply_reminder(reminder, fields);
        Ok(reminder.clone())
    }

    fn delete_reminder(&mut self, id: &str) -> Result<Reminder, Fault> {
        self.note(format!("delete_reminder {id}"));
        Self::take(
            &mut self.reminders,
            |reminder| reminder.id == id,
            "reminder",
        )
    }

    fn contacts(&mut self, query: Option<&str>, page: Page) -> Result<Listing<Contact>, Fault> {
        let found = self
            .contacts
            .iter()
            .filter(|contact| {
                let mut texts = vec![
                    Some(contact.given_name.as_str()),
                    Some(contact.family_name.as_str()),
                ];
                texts.extend(
                    contact
                        .emails
                        .iter()
                        .map(|email| Some(email.value.as_str())),
                );
                mentions(query, &texts)
            })
            .cloned();
        Ok(Listing::gather(found, page))
    }

    fn contact(&mut self, id: &str) -> Result<Contact, Fault> {
        self.contacts
            .iter()
            .find(|contact| contact.id == id)
            .cloned()
            .ok_or_else(|| Fault::Missing(format!("no contact {id}")))
    }

    fn me(&mut self) -> Result<Contact, Fault> {
        self.contacts
            .first()
            .cloned()
            .ok_or_else(|| Fault::Missing("no card is marked as yours".to_owned()))
    }

    fn create_contact(&mut self, fields: ContactFields) -> Result<Contact, Fault> {
        let mut contact = Contact {
            id: self.id("p"),
            ..Contact::default()
        };
        apply_contact(&mut contact, fields);
        self.contacts.push(contact.clone());
        Ok(contact)
    }

    fn update_contact(&mut self, id: &str, fields: ContactFields) -> Result<Contact, Fault> {
        let contact = self
            .contacts
            .iter_mut()
            .find(|contact| contact.id == id)
            .ok_or_else(|| Fault::Missing(format!("no contact {id}")))?;
        apply_contact(contact, fields);
        Ok(contact.clone())
    }

    fn delete_contact(&mut self, id: &str) -> Result<Contact, Fault> {
        self.note(format!("delete_contact {id}"));
        Self::take(&mut self.contacts, |contact| contact.id == id, "contact")
    }
}

fn seconds(moment: Moment) -> i64 {
    match moment {
        Moment::At(at) => at.as_second(),
        Moment::Day(day) => day
            .to_datetime(Time::midnight())
            .to_zoned_datetime(TimeZone::UTC)
            .map_or(0, |at| at.timestamp().as_second()),
    }
}

fn apply_event(event: &mut Event, fields: EventFields) {
    if let Some(calendar) = fields.calendar {
        event.calendar = calendar;
    }
    if let Some(title) = fields.title {
        event.title = title;
    }
    if let Some(start) = fields.start {
        event.start = start;
    }
    if let Some(end) = fields.end {
        event.end = end;
    }
    set(&mut event.location, fields.location);
    set(&mut event.notes, fields.notes);
    set(&mut event.url, fields.url);
    if let Some(alerts) = fields.alerts {
        event.alerts = alerts;
    }
    if let Some(rules) = fields.rules {
        event.rules = rules;
    }
}

fn apply_reminder(reminder: &mut Reminder, fields: ReminderFields) {
    if let Some(list) = fields.list {
        reminder.list = list;
    }
    if let Some(title) = fields.title {
        reminder.title = title;
    }
    set(&mut reminder.notes, fields.notes);
    set(&mut reminder.url, fields.url);
    set(&mut reminder.due, fields.due);
    if let Some(priority) = fields.priority {
        reminder.priority = priority;
    }
    if let Some(is_done) = fields.is_done {
        reminder.is_done = is_done;
    }
    if let Some(alerts) = fields.alerts {
        reminder.alerts = alerts;
    }
    if let Some(rules) = fields.rules {
        reminder.rules = rules;
    }
}

fn apply_contact(contact: &mut Contact, fields: ContactFields) {
    let texts = [
        (&mut contact.given_name, fields.given_name),
        (&mut contact.middle_name, fields.middle_name),
        (&mut contact.family_name, fields.family_name),
        (&mut contact.nickname, fields.nickname),
        (&mut contact.organization, fields.organization),
        (&mut contact.department, fields.department),
        (&mut contact.job_title, fields.job_title),
    ];
    for (slot, value) in texts {
        if let Some(value) = value {
            *slot = value;
        }
    }
    if let Some(emails) = fields.emails {
        contact.emails = emails;
    }
    if let Some(phones) = fields.phones {
        contact.phones = phones;
    }
    if let Some(addresses) = fields.addresses {
        contact.addresses = addresses;
    }
    if let Some(urls) = fields.urls {
        contact.urls = urls;
    }
    set(&mut contact.birthday, fields.birthday);
}

fn set<T>(slot: &mut Option<T>, field: Field<T>) {
    match field {
        Field::Keep => {}
        Field::Clear => *slot = None,
        Field::Set(value) => *slot = Some(value),
    }
}

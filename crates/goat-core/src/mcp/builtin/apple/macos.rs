use std::cell::RefCell;
use std::collections::HashSet;
use std::ptr::NonNull;
use std::sync::mpsc;

use block2::RcBlock;
use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::TimeZone;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AnyThread, Message};
use objc2_contacts::{
    CNAuthorizationStatus, CNContact, CNContactBirthdayKey, CNContactDepartmentNameKey,
    CNContactEmailAddressesKey, CNContactFamilyNameKey, CNContactFetchRequest,
    CNContactGivenNameKey, CNContactIdentifierKey, CNContactJobTitleKey, CNContactMiddleNameKey,
    CNContactNicknameKey, CNContactOrganizationNameKey, CNContactPhoneNumbersKey,
    CNContactPostalAddressesKey, CNContactStore, CNContactUrlAddressesKey, CNEntityType,
    CNKeyDescriptor, CNLabelEmailiCloud, CNLabelHome, CNLabelOther, CNLabelPhoneNumberAppleWatch,
    CNLabelPhoneNumberHomeFax, CNLabelPhoneNumberMain, CNLabelPhoneNumberMobile,
    CNLabelPhoneNumberOtherFax, CNLabelPhoneNumberPager, CNLabelPhoneNumberWorkFax,
    CNLabelPhoneNumberiPhone, CNLabelSchool, CNLabelURLAddressHomePage, CNLabelWork,
    CNLabeledValue, CNMutableContact, CNMutablePostalAddress, CNPhoneNumber, CNPostalAddress,
    CNSaveRequest,
};
use objc2_core_graphics::CGColor;
use objc2_event_kit::{
    EKAlarm, EKAuthorizationStatus, EKCalendar, EKCalendarItem, EKEntityMask, EKEntityType,
    EKEvent, EKEventStore, EKParticipant, EKParticipantStatus, EKRecurrenceDayOfWeek,
    EKRecurrenceEnd, EKRecurrenceFrequency, EKRecurrenceRule, EKReminder, EKReminderPriority,
    EKSpan, EKWeekday,
};
use objc2_foundation::{
    NSArray, NSCopying, NSDate, NSDateComponentUndefined, NSDateComponents, NSError,
    NSMutableCopying, NSNumber, NSPredicate, NSSecureCoding, NSString, NSTimeZone, NSURL,
};

use super::library::{
    Address, Area, Birthday, Calendar, CalendarFields, Color, Contact, ContactFields, Event,
    EventFields, EventQuery, Fault, Field, Kind, Labeled, Library, Listing, Moment, Page, Person,
    Priority, Reminder, ReminderFields, ReminderQuery, Response, Status, mentions,
};
use super::rule::{Day, End, Frequency, Rule, Until};

const AREAS: [Area; 3] = [Area::Calendars, Area::Reminders, Area::Contacts];
const SEARCH_SPAN: i64 = 4 * 365 * 86_400;
const OCCURRENCE: &str = "%Y%m%dT%H%M%S";
const WEEKDAYS: [(Weekday, EKWeekday); 7] = [
    (Weekday::Monday, EKWeekday::Monday),
    (Weekday::Tuesday, EKWeekday::Tuesday),
    (Weekday::Wednesday, EKWeekday::Wednesday),
    (Weekday::Thursday, EKWeekday::Thursday),
    (Weekday::Friday, EKWeekday::Friday),
    (Weekday::Saturday, EKWeekday::Saturday),
    (Weekday::Sunday, EKWeekday::Sunday),
];

type Grant = RcBlock<dyn Fn(Bool, *mut NSError)>;
type SetText = fn(&CNMutableContact, &NSString);

pub(crate) struct Mac {
    store: Retained<EKEventStore>,
    book: Retained<CNContactStore>,
    tz: TimeZone,
}

struct Found {
    event: Retained<EKEvent>,
    span: EKSpan,
}

impl Mac {
    pub(crate) fn new() -> Result<Self, Fault> {
        if !AREAS.into_iter().any(is_granted) {
            return Err(Fault::Denied(
                "goat has no Calendars, Reminders or Contacts access; run goat plugin enable apple"
                    .to_owned(),
            ));
        }
        Ok(Self {
            store: unsafe { EKEventStore::new() },
            book: unsafe { CNContactStore::new() },
            tz: TimeZone::get(&NSTimeZone::localTimeZone().name().to_string())
                .unwrap_or(TimeZone::UTC),
        })
    }

    fn refresh(&self) {
        unsafe { self.store.reset() };
    }

    fn calendar(&self, id: &str) -> Result<Retained<EKCalendar>, Fault> {
        unsafe { self.store.calendarWithIdentifier(&NSString::from_str(id)) }
            .ok_or_else(|| Fault::Missing(format!("no calendar or list {id}")))
    }

    fn default_calendar(&self, kind: Kind) -> Option<Retained<EKCalendar>> {
        match kind {
            Kind::Events => unsafe { self.store.defaultCalendarForNewEvents() },
            Kind::Reminders => unsafe { self.store.defaultCalendarForNewReminders() },
        }
    }

    fn calendar_or_default(
        &self,
        id: Option<&str>,
        kind: Kind,
    ) -> Result<Retained<EKCalendar>, Fault> {
        match id {
            Some(id) => self.calendar(id),
            None => self
                .default_calendar(kind)
                .ok_or_else(|| Fault::Missing("there is no default calendar or list".to_owned())),
        }
    }

    fn pick(&self, ids: &[String]) -> Result<Option<Retained<NSArray<EKCalendar>>>, Fault> {
        if ids.is_empty() {
            return Ok(None);
        }
        let calendars = ids
            .iter()
            .map(|id| self.calendar(id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(NSArray::from_retained_slice(&calendars)))
    }

    fn describe_calendar(&self, calendar: &EKCalendar) -> Calendar {
        let mask = unsafe { calendar.allowedEntityTypes() };
        let kind = if mask.contains(EKEntityMask::Reminder) {
            Kind::Reminders
        } else {
            Kind::Events
        };
        let id = unsafe { calendar.calendarIdentifier() }.to_string();
        let default = self
            .default_calendar(kind)
            .map(|default| unsafe { default.calendarIdentifier() }.to_string());
        Calendar {
            is_default: default.as_deref() == Some(id.as_str()),
            id,
            title: unsafe { calendar.title() }.to_string(),
            kind,
            account: unsafe { calendar.source() }
                .map_or_default(|source| unsafe { source.title() }.to_string()),
            color: unsafe { calendar.CGColor() }.and_then(|color| read_color(&color)),
            is_writable: unsafe { calendar.allowsContentModifications() },
        }
    }

    fn find_event(&self, id: &str) -> Result<Found, Fault> {
        let missing = || Fault::Missing(format!("no event {id}"));
        if let Some((base, at)) = id
            .rsplit_once('_')
            .and_then(|(base, at)| Some((base, occurrence_time(at)?)))
        {
            let predicate = unsafe {
                self.store
                    .predicateForEventsWithStartDate_endDate_calendars(
                        &ns_date(at.as_second()),
                        &ns_date(at.as_second() + 1),
                        None,
                    )
            };
            let event = unsafe { self.store.eventsMatchingPredicate(&predicate) }
                .iter()
                .find(|event| {
                    unsafe { event.eventIdentifier() }
                        .is_some_and(|known| known.to_string() == base)
                        && start_of(event) == at.as_second()
                })
                .ok_or_else(missing)?;
            return Ok(Found {
                event,
                span: EKSpan::ThisEvent,
            });
        }
        let event = unsafe { self.store.eventWithIdentifier(&NSString::from_str(id)) }
            .ok_or_else(missing)?;
        let span = if unsafe { event.hasRecurrenceRules() } {
            EKSpan::FutureEvents
        } else {
            EKSpan::ThisEvent
        };
        Ok(Found { event, span })
    }

    fn apply_event(&self, event: &EKEvent, fields: EventFields) -> Result<(), Fault> {
        if let Some(id) = &fields.calendar {
            let calendar = self.calendar(id)?;
            unsafe { event.setCalendar(Some(&calendar)) };
        }
        if let Some(title) = &fields.title {
            unsafe { event.setTitle(Some(&NSString::from_str(title))) };
        }
        if let Some(start) = fields.start {
            unsafe { event.setAllDay(matches!(start, Moment::Day(_))) };
            unsafe { event.setStartDate(Some(&ns_date(self.start_second(start)?))) };
        }
        if let Some(end) = fields.end {
            unsafe { event.setEndDate(Some(&ns_date(self.end_second(end)?))) };
        }
        set_text(fields.location, |text| unsafe { event.setLocation(text) });
        set_text(fields.notes, |text| unsafe { event.setNotes(text) });
        set_url(fields.url, |url| unsafe { event.setURL(url) })?;
        if let Some(alerts) = &fields.alerts {
            unsafe { event.setAlarms(Some(&alarms(alerts))) };
        }
        if let Some(rules) = &fields.rules {
            self.set_rules(rules, event)?;
        }
        Ok(())
    }

    fn describe_event(&self, event: &EKEvent) -> Event {
        let (begin, finish) = (start_of(event), end_of(event));
        let (start, end) = if unsafe { event.isAllDay() } {
            let first = self.local_date(begin);
            let after = self
                .local_date((finish - 1).max(begin))
                .tomorrow()
                .unwrap_or(first);
            (Moment::Day(first), Moment::Day(after.max(first)))
        } else {
            (Moment::At(timestamp(begin)), Moment::At(timestamp(finish)))
        };
        let base = unsafe { event.eventIdentifier() }.map_or_default(|id| id.to_string());
        let is_repeating = unsafe { event.hasRecurrenceRules() } || unsafe { event.isDetached() };
        Event {
            id: if is_repeating {
                format!("{base}_{}Z", timestamp(begin).strftime(OCCURRENCE))
            } else {
                base.clone()
            },
            series: is_repeating.then_some(base),
            calendar: calendar_id(event),
            title: unsafe { event.title() }.to_string(),
            start,
            end,
            location: text(unsafe { event.location() }),
            notes: text(unsafe { event.notes() }),
            url: url_text(event),
            alerts: alerts(event, Some(begin)),
            rules: rules(event),
            organizer: unsafe { event.organizer() }.map(|person| describe_person(&person)),
            attendees: unsafe { event.attendees() }.map_or_default(|people| {
                people
                    .iter()
                    .map(|person| describe_person(&person))
                    .collect()
            }),
        }
    }

    fn find_reminder(&self, id: &str) -> Result<Retained<EKReminder>, Fault> {
        unsafe {
            self.store
                .calendarItemWithIdentifier(&NSString::from_str(id))
        }
        .and_then(|item| item.downcast::<EKReminder>().ok())
        .ok_or_else(|| Fault::Missing(format!("no reminder {id}")))
    }

    fn fetch_reminders(&self, predicate: &NSPredicate) -> Result<Vec<Reminder>, Fault> {
        let (send, receive) = mpsc::channel();
        let tz = self.tz.clone();
        let block = RcBlock::new(move |found: *mut NSArray<EKReminder>| {
            let reminders: Vec<Reminder> = unsafe { found.as_ref() }.map_or_default(|found| {
                found
                    .iter()
                    .map(|item| describe_reminder(&item, &tz))
                    .collect()
            });
            send.send(reminders).ok();
        });
        unsafe {
            self.store
                .fetchRemindersMatchingPredicate_completion(predicate, &block)
        };
        receive
            .recv()
            .map_err(|_| Fault::Failed("Reminders did not answer".to_owned()))
    }

    fn apply_reminder(&self, reminder: &EKReminder, fields: ReminderFields) -> Result<(), Fault> {
        if let Some(id) = &fields.list {
            let list = self.calendar(id)?;
            unsafe { reminder.setCalendar(Some(&list)) };
        }
        if let Some(title) = &fields.title {
            unsafe { reminder.setTitle(Some(&NSString::from_str(title))) };
        }
        set_text(fields.notes, |text| unsafe { reminder.setNotes(text) });
        set_url(fields.url, |url| unsafe { reminder.setURL(url) })?;
        match fields.due {
            Field::Keep => {}
            Field::Clear => unsafe { reminder.setDueDateComponents(None) },
            Field::Set(due) => {
                let parts = self.components(due);
                unsafe { reminder.setDueDateComponents(Some(&parts)) };
            }
        }
        if let Some(priority) = fields.priority {
            let value = match priority {
                Priority::None => EKReminderPriority::None,
                Priority::High => EKReminderPriority::High,
                Priority::Medium => EKReminderPriority::Medium,
                Priority::Low => EKReminderPriority::Low,
            };
            unsafe { reminder.setPriority(value.0) };
        }
        if let Some(is_done) = fields.is_done {
            unsafe { reminder.setCompleted(is_done) };
        }
        if let Some(alerts) = &fields.alerts {
            unsafe { reminder.setAlarms(Some(&alarms(alerts))) };
        }
        if let Some(rules) = &fields.rules {
            self.set_rules(rules, reminder)?;
        }
        Ok(())
    }

    fn set_rules(&self, rules: &[Rule], item: &EKCalendarItem) -> Result<(), Fault> {
        if rules.is_empty() {
            unsafe { item.setRecurrenceRules(None) };
            return Ok(());
        }
        let rules = rules
            .iter()
            .map(|rule| self.write_rule(rule))
            .collect::<Result<Vec<_>, _>>()?;
        unsafe { item.setRecurrenceRules(Some(&NSArray::from_retained_slice(&rules))) };
        Ok(())
    }

    fn write_rule(&self, rule: &Rule) -> Result<Retained<EKRecurrenceRule>, Fault> {
        let frequency = match rule.frequency {
            Frequency::Daily => EKRecurrenceFrequency::Daily,
            Frequency::Weekly => EKRecurrenceFrequency::Weekly,
            Frequency::Monthly => EKRecurrenceFrequency::Monthly,
            Frequency::Yearly => EKRecurrenceFrequency::Yearly,
        };
        let days = (!rule.days.is_empty()).then(|| {
            let days: Vec<_> = rule
                .days
                .iter()
                .map(|day| unsafe {
                    EKRecurrenceDayOfWeek::dayOfWeek_weekNumber(
                        ek_weekday(day.weekday),
                        isize::from(day.week),
                    )
                })
                .collect();
            NSArray::from_retained_slice(&days)
        });
        let month_days = numbers(rule.month_days.iter().copied().map(isize::from));
        let months = numbers(rule.months.iter().copied().map(isize::from));
        let positions = numbers(rule.positions.iter().copied().map(isize::from));
        let end = match rule.end {
            None => None,
            Some(End::Count(count)) => {
                Some(unsafe { EKRecurrenceEnd::recurrenceEndWithOccurrenceCount(count as usize) })
            }
            Some(End::Until(until)) => {
                let second = match until {
                    Until::At(at) => at.as_second(),
                    Until::Floating(time) => self.local_second(time)?,
                    Until::Day(day) => self.end_second(Moment::Day(
                        day.tomorrow()
                            .map_err(|err| Fault::Failed(err.to_string()))?,
                    ))?,
                };
                Some(unsafe { EKRecurrenceEnd::recurrenceEndWithEndDate(&ns_date(second)) })
            }
        };
        Ok(unsafe {
            EKRecurrenceRule::initRecurrenceWithFrequency_interval_daysOfTheWeek_daysOfTheMonth_monthsOfTheYear_weeksOfTheYear_daysOfTheYear_setPositions_end(
                EKRecurrenceRule::alloc(),
                frequency,
                isize::try_from(rule.interval).unwrap_or(1),
                days.as_deref(),
                month_days.as_deref(),
                months.as_deref(),
                None,
                None,
                positions.as_deref(),
                end.as_deref(),
            )
        })
    }

    fn components(&self, due: Moment) -> Retained<NSDateComponents> {
        let parts = NSDateComponents::new();
        let (date, time) = match due {
            Moment::Day(day) => (day, None),
            Moment::At(at) => {
                let local = at.to_zoned_datetime(self.tz.clone());
                (local.date(), Some(local.time()))
            }
        };
        parts.setYear(isize::from(date.year()));
        parts.setMonth(isize::from(date.month()));
        parts.setDay(isize::from(date.day()));
        if let Some(time) = time {
            parts.setHour(isize::from(time.hour()));
            parts.setMinute(isize::from(time.minute()));
            parts.setSecond(isize::from(time.second()));
            parts.setTimeZone(Some(&NSTimeZone::localTimeZone()));
        }
        parts
    }

    fn start_second(&self, moment: Moment) -> Result<i64, Fault> {
        match moment {
            Moment::At(at) => Ok(at.as_second()),
            Moment::Day(day) => self.local_second(day.to_datetime(Time::midnight())),
        }
    }

    fn end_second(&self, moment: Moment) -> Result<i64, Fault> {
        match moment {
            Moment::At(at) => Ok(at.as_second()),
            Moment::Day(after) => Ok(self.start_second(Moment::Day(after))? - 1),
        }
    }

    fn local_second(&self, time: DateTime) -> Result<i64, Fault> {
        time.to_zoned_datetime(self.tz.clone())
            .map(|at| at.timestamp().as_second())
            .map_err(|err| Fault::Failed(err.to_string()))
    }

    fn local_date(&self, second: i64) -> Date {
        timestamp(second).to_zoned_datetime(self.tz.clone()).date()
    }

    fn unified(&self, id: &str) -> Result<Retained<CNContact>, Fault> {
        unsafe {
            self.book
                .unifiedContactWithIdentifier_keysToFetch_error(&NSString::from_str(id), &keys())
        }
        .map_err(|_| Fault::Missing(format!("no contact {id}")))
    }

    fn save(&self, change: impl FnOnce(&CNSaveRequest)) -> Result<(), Fault> {
        let request = unsafe { CNSaveRequest::new() };
        change(&request);
        unsafe { self.book.executeSaveRequest_error(&request) }.map_err(|err| failed(&err))
    }

    fn every_contact(&self, page: Page) -> Result<Listing<Contact>, Fault> {
        let request = unsafe {
            CNContactFetchRequest::initWithKeysToFetch(CNContactFetchRequest::alloc(), &keys())
        };
        let found = RefCell::new(Vec::new());
        let enough = page.skip + page.limit;
        let block = RcBlock::new(|contact: NonNull<CNContact>, stop: NonNull<Bool>| {
            let mut found = found.borrow_mut();
            found.push(describe_contact(unsafe { contact.as_ref() }));
            if found.len() > enough {
                unsafe { stop.write(Bool::YES) };
            }
        });
        let mut error = None;
        let is_done = unsafe {
            self.book
                .enumerateContactsWithFetchRequest_error_usingBlock(
                    &request,
                    Some(&mut error),
                    &block,
                )
        };
        drop(block);
        if !is_done && let Some(err) = error {
            return Err(failed(&err));
        }
        Ok(Listing::gather(found.into_inner(), page))
    }
}

impl Library for Mac {
    fn granted(&mut self) -> Vec<Area> {
        AREAS.into_iter().filter(|area| is_granted(*area)).collect()
    }

    fn time_zone(&mut self) -> String {
        self.tz.iana_name().unwrap_or("UTC").to_owned()
    }

    fn calendars(&mut self) -> Result<Vec<Calendar>, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let mut found = Vec::new();
            for (entity, area) in [
                (EKEntityType::Event, Area::Calendars),
                (EKEntityType::Reminder, Area::Reminders),
            ] {
                if !is_granted(area) {
                    continue;
                }
                let calendars = unsafe { self.store.calendarsForEntityType(entity) };
                found.extend(
                    calendars
                        .iter()
                        .map(|calendar| self.describe_calendar(&calendar)),
                );
            }
            Ok(found)
        })
    }

    fn create_calendar(&mut self, fields: CalendarFields) -> Result<Calendar, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let kind = fields.kind.unwrap_or(Kind::Events);
            let entity = match kind {
                Kind::Events => EKEntityType::Event,
                Kind::Reminders => EKEntityType::Reminder,
            };
            let source = match &fields.account {
                Some(account) => unsafe { self.store.sources() }
                    .iter()
                    .find(|source| {
                        unsafe { source.title() }
                            .to_string()
                            .eq_ignore_ascii_case(account)
                    })
                    .ok_or_else(|| Fault::Missing(format!("no account named {account}")))?,
                None => self
                    .default_calendar(kind)
                    .and_then(|calendar| unsafe { calendar.source() })
                    .ok_or_else(|| Fault::Missing("there is no default account".to_owned()))?,
            };
            let calendar =
                unsafe { EKCalendar::calendarForEntityType_eventStore(entity, &self.store) };
            unsafe { calendar.setSource(Some(&source)) };
            unsafe { calendar.setTitle(&NSString::from_str(&fields.title.unwrap_or_default())) };
            if let Some(color) = fields.color {
                unsafe { calendar.setCGColor(Some(&write_color(color))) };
            }
            unsafe { self.store.saveCalendar_commit_error(&calendar, true) }
                .map_err(|err| failed(&err))?;
            Ok(self.describe_calendar(&calendar))
        })
    }

    fn update_calendar(&mut self, id: &str, fields: CalendarFields) -> Result<Calendar, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let calendar = self.calendar(id)?;
            if let Some(title) = &fields.title {
                unsafe { calendar.setTitle(&NSString::from_str(title)) };
            }
            if let Some(color) = fields.color {
                unsafe { calendar.setCGColor(Some(&write_color(color))) };
            }
            unsafe { self.store.saveCalendar_commit_error(&calendar, true) }
                .map_err(|err| failed(&err))?;
            Ok(self.describe_calendar(&calendar))
        })
    }

    fn delete_calendar(&mut self, id: &str) -> Result<Calendar, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let calendar = self.calendar(id)?;
            let described = self.describe_calendar(&calendar);
            unsafe { self.store.removeCalendar_commit_error(&calendar, true) }
                .map_err(|err| failed(&err))?;
            Ok(described)
        })
    }

    fn events(&mut self, query: &EventQuery) -> Result<Listing<Event>, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let calendars = self.pick(&query.calendars)?;
            let end = query.to.as_second();
            let enough = query.page.skip + query.page.limit;
            let mut found = Vec::new();
            let mut seen = HashSet::new();
            let mut from = query.from.as_second();
            while from <= end && found.len() <= enough {
                let to = (from + SEARCH_SPAN).min(end);
                let predicate = unsafe {
                    self.store
                        .predicateForEventsWithStartDate_endDate_calendars(
                            &ns_date(from),
                            &ns_date(to),
                            calendars.as_deref(),
                        )
                };
                let mut events: Vec<Event> =
                    unsafe { self.store.eventsMatchingPredicate(&predicate) }
                        .iter()
                        .map(|event| self.describe_event(&event))
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
                        .collect();
                events.sort_by_key(|event| self.start_second(event.start).unwrap_or_default());
                found.extend(
                    events
                        .into_iter()
                        .filter(|event| seen.insert(event.id.clone())),
                );
                from = to + 1;
            }
            Ok(Listing::gather(found, query.page))
        })
    }

    fn event(&mut self, id: &str) -> Result<Event, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let found = self.find_event(id)?;
            Ok(self.describe_event(&found.event))
        })
    }

    fn create_event(&mut self, fields: EventFields) -> Result<Event, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let event = unsafe { EKEvent::eventWithEventStore(&self.store) };
            let calendar = self.calendar_or_default(fields.calendar.as_deref(), Kind::Events)?;
            unsafe { event.setCalendar(Some(&calendar)) };
            self.apply_event(&event, fields)?;
            unsafe {
                self.store
                    .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
            }
            .map_err(|err| failed(&err))?;
            Ok(self.describe_event(&event))
        })
    }

    fn update_event(&mut self, id: &str, fields: EventFields) -> Result<Event, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let found = self.find_event(id)?;
            self.apply_event(&found.event, fields)?;
            unsafe {
                self.store
                    .saveEvent_span_commit_error(&found.event, found.span, true)
            }
            .map_err(|err| failed(&err))?;
            Ok(self.describe_event(&found.event))
        })
    }

    fn delete_event(&mut self, id: &str) -> Result<Event, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let found = self.find_event(id)?;
            let described = self.describe_event(&found.event);
            unsafe {
                self.store
                    .removeEvent_span_commit_error(&found.event, found.span, true)
            }
            .map_err(|err| failed(&err))?;
            Ok(described)
        })
    }

    fn reminders(&mut self, query: &ReminderQuery) -> Result<Listing<Reminder>, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let lists = self.pick(&query.lists)?;
            let predicate = match query.status {
                Status::Open => unsafe {
                    self.store
                        .predicateForIncompleteRemindersWithDueDateStarting_ending_calendars(
                            query.from.map(|from| ns_date(from.as_second())).as_deref(),
                            query.to.map(|to| ns_date(to.as_second())).as_deref(),
                            lists.as_deref(),
                        )
                },
                Status::Done => unsafe {
                    self.store
                        .predicateForCompletedRemindersWithCompletionDateStarting_ending_calendars(
                            None,
                            None,
                            lists.as_deref(),
                        )
                },
                Status::All => unsafe {
                    self.store
                        .predicateForRemindersInCalendars(lists.as_deref())
                },
            };
            let due =
                |reminder: &Reminder| reminder.due.and_then(|due| self.start_second(due).ok());
            let (from, to) = (
                query.from.map(Timestamp::as_second),
                query.to.map(Timestamp::as_second),
            );
            let mut found: Vec<Reminder> = self
                .fetch_reminders(&predicate)?
                .into_iter()
                .filter(|reminder| {
                    let at = due(reminder);
                    from.is_none_or(|from| at.is_some_and(|at| at >= from))
                        && to.is_none_or(|to| at.is_some_and(|at| at <= to))
                })
                .filter(|reminder| {
                    mentions(
                        query.text.as_deref(),
                        &[Some(&reminder.title), reminder.notes.as_deref()],
                    )
                })
                .collect();
            found.sort_by_key(|reminder| due(reminder).unwrap_or(i64::MAX));
            Ok(Listing::gather(found, query.page))
        })
    }

    fn create_reminder(&mut self, fields: ReminderFields) -> Result<Reminder, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let reminder = unsafe { EKReminder::reminderWithEventStore(&self.store) };
            let list = self.calendar_or_default(fields.list.as_deref(), Kind::Reminders)?;
            unsafe { reminder.setCalendar(Some(&list)) };
            self.apply_reminder(&reminder, fields)?;
            unsafe { self.store.saveReminder_commit_error(&reminder, true) }
                .map_err(|err| failed(&err))?;
            Ok(describe_reminder(&reminder, &self.tz))
        })
    }

    fn update_reminder(&mut self, id: &str, fields: ReminderFields) -> Result<Reminder, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let reminder = self.find_reminder(id)?;
            self.apply_reminder(&reminder, fields)?;
            unsafe { self.store.saveReminder_commit_error(&reminder, true) }
                .map_err(|err| failed(&err))?;
            Ok(describe_reminder(&reminder, &self.tz))
        })
    }

    fn delete_reminder(&mut self, id: &str) -> Result<Reminder, Fault> {
        autoreleasepool(|_| {
            self.refresh();
            let reminder = self.find_reminder(id)?;
            let described = describe_reminder(&reminder, &self.tz);
            unsafe { self.store.removeReminder_commit_error(&reminder, true) }
                .map_err(|err| failed(&err))?;
            Ok(described)
        })
    }

    fn contacts(&mut self, query: Option<&str>, page: Page) -> Result<Listing<Contact>, Fault> {
        autoreleasepool(|_| {
            let Some(query) = query else {
                return self.every_contact(page);
            };
            let predicate = if query.contains('@') {
                unsafe {
                    CNContact::predicateForContactsMatchingEmailAddress(&NSString::from_str(query))
                }
            } else if let Some(number) = phone(query) {
                unsafe { CNContact::predicateForContactsMatchingPhoneNumber(&number) }
            } else {
                unsafe { CNContact::predicateForContactsMatchingName(&NSString::from_str(query)) }
            };
            let found = unsafe {
                self.book
                    .unifiedContactsMatchingPredicate_keysToFetch_error(&predicate, &keys())
            }
            .map_err(|err| failed(&err))?;
            Ok(Listing::gather(
                found.iter().map(|contact| describe_contact(&contact)),
                page,
            ))
        })
    }

    fn contact(&mut self, id: &str) -> Result<Contact, Fault> {
        autoreleasepool(|_| self.unified(id).map(|contact| describe_contact(&contact)))
    }

    fn me(&mut self) -> Result<Contact, Fault> {
        autoreleasepool(|_| {
            unsafe { self.book.unifiedMeContactWithKeysToFetch_error(&keys()) }
                .map(|contact| describe_contact(&contact))
                .map_err(|_| Fault::Missing("no contact card is marked as yours".to_owned()))
        })
    }

    fn create_contact(&mut self, fields: ContactFields) -> Result<Contact, Fault> {
        autoreleasepool(|_| {
            let contact = unsafe { CNMutableContact::new() };
            apply_contact(&contact, fields);
            self.save(|request| unsafe {
                request.addContact_toContainerWithIdentifier(&contact, None);
            })?;
            let id = unsafe { contact.identifier() }.to_string();
            self.unified(&id).map(|contact| describe_contact(&contact))
        })
    }

    fn update_contact(&mut self, id: &str, fields: ContactFields) -> Result<Contact, Fault> {
        autoreleasepool(|_| {
            let contact = self.unified(id)?.mutableCopy();
            apply_contact(&contact, fields);
            self.save(|request| unsafe { request.updateContact(&contact) })?;
            self.unified(id).map(|contact| describe_contact(&contact))
        })
    }

    fn delete_contact(&mut self, id: &str) -> Result<Contact, Fault> {
        autoreleasepool(|_| {
            let existing = self.unified(id)?;
            let described = describe_contact(&existing);
            let contact = existing.mutableCopy();
            self.save(|request| unsafe { request.deleteContact(&contact) })?;
            Ok(described)
        })
    }
}

pub(crate) fn missing(ask: bool) -> Vec<&'static str> {
    if ask {
        request_access();
    }
    AREAS
        .into_iter()
        .filter(|area| !is_granted(*area))
        .map(Area::settings_name)
        .collect()
}

fn request_access() {
    let store = unsafe { EKEventStore::new() };
    if event_status(EKEntityType::Event) == EKAuthorizationStatus::NotDetermined {
        wait(|done| unsafe {
            store.requestFullAccessToEventsWithCompletion(RcBlock::as_ptr(done));
        });
    }
    if event_status(EKEntityType::Reminder) == EKAuthorizationStatus::NotDetermined {
        wait(|done| unsafe {
            store.requestFullAccessToRemindersWithCompletion(RcBlock::as_ptr(done));
        });
    }
    if contact_status() == CNAuthorizationStatus::NotDetermined {
        let book = unsafe { CNContactStore::new() };
        wait(|done| unsafe {
            book.requestAccessForEntityType_completionHandler(CNEntityType::Contacts, done);
        });
    }
}

fn wait(ask: impl FnOnce(&Grant)) {
    let (send, receive) = mpsc::channel();
    let done: Grant = RcBlock::new(move |_: Bool, _: *mut NSError| {
        send.send(()).ok();
    });
    ask(&done);
    receive.recv().ok();
}

fn is_granted(area: Area) -> bool {
    match area {
        Area::Calendars => event_status(EKEntityType::Event) == EKAuthorizationStatus::FullAccess,
        Area::Reminders => {
            event_status(EKEntityType::Reminder) == EKAuthorizationStatus::FullAccess
        }
        Area::Contacts => matches!(
            contact_status(),
            CNAuthorizationStatus::Authorized | CNAuthorizationStatus::Limited
        ),
    }
}

fn event_status(entity: EKEntityType) -> EKAuthorizationStatus {
    unsafe { EKEventStore::authorizationStatusForEntityType(entity) }
}

fn contact_status() -> CNAuthorizationStatus {
    unsafe { CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts) }
}

fn start_of(event: &EKEvent) -> i64 {
    let start = unsafe { event.startDate() };
    seconds(&start)
}

fn end_of(event: &EKEvent) -> i64 {
    let end = unsafe { event.endDate() };
    seconds(&end)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "seconds since 1970 fit in i64 for any calendar date"
)]
fn seconds(date: &NSDate) -> i64 {
    date.timeIntervalSince1970().floor() as i64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "seconds since 1970 stay far below 2^53"
)]
fn ns_date(second: i64) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(second as f64)
}

fn timestamp(second: i64) -> Timestamp {
    Timestamp::from_second(second).unwrap_or(Timestamp::UNIX_EPOCH)
}

fn occurrence_time(text: &str) -> Option<Timestamp> {
    let time = DateTime::strptime(OCCURRENCE, text.strip_suffix('Z')?).ok()?;
    time.to_zoned_datetime(TimeZone::UTC)
        .ok()
        .map(|at| at.timestamp())
}

fn when_of(parts: &NSDateComponents, tz: &TimeZone) -> Option<Moment> {
    let part = |value: isize| i8::try_from(value).ok();
    let date = Date::new(
        i16::try_from(parts.year()).ok()?,
        part(parts.month())?,
        part(parts.day())?,
    )
    .ok()?;
    if parts.hour() == NSDateComponentUndefined {
        return Some(Moment::Day(date));
    }
    let clock = |value: isize| {
        if value == NSDateComponentUndefined {
            Some(0)
        } else {
            part(value)
        }
    };
    let time = Time::new(
        part(parts.hour())?,
        clock(parts.minute())?,
        clock(parts.second())?,
        0,
    )
    .ok()?;
    let zone = parts
        .timeZone()
        .and_then(|zone| TimeZone::get(&zone.name().to_string()).ok())
        .unwrap_or_else(|| tz.clone());
    date.to_datetime(time)
        .to_zoned_datetime(zone)
        .ok()
        .map(|at| Moment::At(at.timestamp()))
}

fn calendar_id(item: &EKCalendarItem) -> String {
    unsafe { item.calendar() }
        .map_or_default(|calendar| unsafe { calendar.calendarIdentifier() }.to_string())
}

fn text(value: Option<Retained<NSString>>) -> Option<String> {
    value
        .map(|text| text.to_string())
        .filter(|text| !text.is_empty())
}

fn url_text(item: &EKCalendarItem) -> Option<String> {
    unsafe { item.URL() }
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string())
}

fn set_text(field: Field<String>, set: impl FnOnce(Option<&NSString>)) {
    match field {
        Field::Keep => {}
        Field::Clear => set(None),
        Field::Set(text) => set(Some(&NSString::from_str(&text))),
    }
}

fn set_url(field: Field<String>, set: impl FnOnce(Option<&NSURL>)) -> Result<(), Fault> {
    match field {
        Field::Keep => {}
        Field::Clear => set(None),
        Field::Set(text) => {
            let url = NSURL::URLWithString(&NSString::from_str(&text))
                .ok_or_else(|| Fault::Failed(format!("bad arguments: {text} is not a URL")))?;
            set(Some(&url));
        }
    }
    Ok(())
}

#[expect(
    clippy::cast_precision_loss,
    reason = "alert offsets are minutes, far below 2^53"
)]
fn alarms(minutes: &[i64]) -> Retained<NSArray<EKAlarm>> {
    let alarms: Vec<_> = minutes
        .iter()
        .map(|minutes| unsafe { EKAlarm::alarmWithRelativeOffset(-(minutes * 60) as f64) })
        .collect();
    NSArray::from_retained_slice(&alarms)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "alert offsets are whole minutes"
)]
fn alerts(item: &EKCalendarItem, reference: Option<i64>) -> Vec<i64> {
    let Some(alarms) = (unsafe { item.alarms() }) else {
        return Vec::new();
    };
    alarms
        .iter()
        .filter_map(|alarm| match unsafe { alarm.absoluteDate() } {
            Some(at) => reference.map(|reference| (reference - seconds(&at)) / 60),
            None => Some((-unsafe { alarm.relativeOffset() } / 60.0).round() as i64),
        })
        .collect()
}

fn numbers(values: impl Iterator<Item = isize>) -> Option<Retained<NSArray<NSNumber>>> {
    let numbers: Vec<_> = values.map(NSNumber::new_isize).collect();
    (!numbers.is_empty()).then(|| NSArray::from_retained_slice(&numbers))
}

fn rules(item: &EKCalendarItem) -> Vec<Rule> {
    unsafe { item.recurrenceRules() }
        .map_or_default(|rules| rules.iter().map(|rule| read_rule(&rule)).collect())
}

fn read_rule(found: &EKRecurrenceRule) -> Rule {
    let frequency = match unsafe { found.frequency() } {
        EKRecurrenceFrequency::Weekly => Frequency::Weekly,
        EKRecurrenceFrequency::Monthly => Frequency::Monthly,
        EKRecurrenceFrequency::Yearly => Frequency::Yearly,
        _ => Frequency::Daily,
    };
    let mut rule = Rule::new(frequency);
    rule.interval = u32::try_from(unsafe { found.interval() })
        .unwrap_or(1)
        .max(1);
    rule.days = unsafe { found.daysOfTheWeek() }.map_or_default(|days| {
        days.iter()
            .filter_map(|day| {
                let weekday = unsafe { day.dayOfTheWeek() };
                Some(Day {
                    weekday: WEEKDAYS.iter().find(|(_, known)| *known == weekday)?.0,
                    week: i8::try_from(unsafe { day.weekNumber() }).unwrap_or(0),
                })
            })
            .collect()
    });
    rule.month_days = read_numbers(unsafe { found.daysOfTheMonth() });
    rule.months = read_numbers(unsafe { found.monthsOfTheYear() });
    rule.positions = read_numbers(unsafe { found.setPositions() });
    rule.end = unsafe { found.recurrenceEnd() }.map(|end| match unsafe { end.endDate() } {
        Some(until) => End::Until(Until::At(timestamp(seconds(&until)))),
        None => End::Count(u32::try_from(unsafe { end.occurrenceCount() }).unwrap_or(1)),
    });
    rule
}

fn read_numbers<T: TryFrom<isize>>(numbers: Option<Retained<NSArray<NSNumber>>>) -> Vec<T> {
    numbers.map_or_default(|numbers| {
        numbers
            .iter()
            .filter_map(|number| T::try_from(number.as_isize()).ok())
            .collect()
    })
}

fn ek_weekday(weekday: Weekday) -> EKWeekday {
    WEEKDAYS
        .iter()
        .find(|(known, _)| *known == weekday)
        .map_or(EKWeekday::Monday, |(_, day)| *day)
}

fn describe_person(person: &EKParticipant) -> Person {
    let response = match unsafe { person.participantStatus() } {
        EKParticipantStatus::Accepted => Response::Accepted,
        EKParticipantStatus::Declined => Response::Declined,
        EKParticipantStatus::Tentative => Response::Tentative,
        _ => Response::NeedsAction,
    };
    let address = unsafe { person.URL() }
        .absoluteString()
        .map(|url| url.to_string());
    Person {
        name: text(unsafe { person.name() }),
        email: address.and_then(|url| url.strip_prefix("mailto:").map(str::to_owned)),
        response,
        is_self: unsafe { person.isCurrentUser() },
    }
}

fn describe_reminder(item: &EKReminder, tz: &TimeZone) -> Reminder {
    let due = unsafe { item.dueDateComponents() }.and_then(|parts| when_of(&parts, tz));
    let reference = due.and_then(|due| match due {
        Moment::At(at) => Some(at.as_second()),
        Moment::Day(day) => day
            .to_datetime(Time::midnight())
            .to_zoned_datetime(tz.clone())
            .ok()
            .map(|at| at.timestamp().as_second()),
    });
    let priority = match unsafe { item.priority() } {
        0 => Priority::None,
        1..=4 => Priority::High,
        5 => Priority::Medium,
        _ => Priority::Low,
    };
    Reminder {
        id: unsafe { item.calendarItemIdentifier() }.to_string(),
        list: calendar_id(item),
        title: unsafe { item.title() }.to_string(),
        notes: text(unsafe { item.notes() }),
        url: url_text(item),
        due,
        priority,
        is_done: unsafe { item.isCompleted() },
        completed: unsafe { item.completionDate() }.map(|at| timestamp(seconds(&at))),
        alerts: alerts(item, reference),
        rules: rules(item),
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "each channel is clamped to 0..=255 before the cast"
)]
fn read_color(color: &CGColor) -> Option<Color> {
    let count = CGColor::number_of_components(Some(color));
    let start = CGColor::components(Some(color));
    if start.is_null() || count < 2 {
        return None;
    }
    let parts = unsafe { std::slice::from_raw_parts(start, count) };
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (red, green, blue) = if count >= 4 {
        (parts[0], parts[1], parts[2])
    } else {
        (parts[0], parts[0], parts[0])
    };
    Some(Color {
        red: channel(red),
        green: channel(green),
        blue: channel(blue),
    })
}

fn write_color(color: Color) -> objc2_core_foundation::CFRetained<CGColor> {
    let channel = |value: u8| f64::from(value) / 255.0;
    CGColor::new_srgb(
        channel(color.red),
        channel(color.green),
        channel(color.blue),
        1.0,
    )
}

fn keys() -> Retained<NSArray<ProtocolObject<dyn CNKeyDescriptor>>> {
    let names: [&NSString; 13] = unsafe {
        [
            CNContactIdentifierKey,
            CNContactGivenNameKey,
            CNContactMiddleNameKey,
            CNContactFamilyNameKey,
            CNContactNicknameKey,
            CNContactOrganizationNameKey,
            CNContactDepartmentNameKey,
            CNContactJobTitleKey,
            CNContactEmailAddressesKey,
            CNContactPhoneNumbersKey,
            CNContactPostalAddressesKey,
            CNContactUrlAddressesKey,
            CNContactBirthdayKey,
        ]
    };
    let keys: Vec<&ProtocolObject<dyn CNKeyDescriptor>> =
        names.into_iter().map(ProtocolObject::from_ref).collect();
    NSArray::from_slice(&keys)
}

fn known_labels() -> [&'static NSString; 14] {
    unsafe {
        [
            CNLabelHome,
            CNLabelWork,
            CNLabelSchool,
            CNLabelOther,
            CNLabelEmailiCloud,
            CNLabelURLAddressHomePage,
            CNLabelPhoneNumberiPhone,
            CNLabelPhoneNumberAppleWatch,
            CNLabelPhoneNumberMobile,
            CNLabelPhoneNumberMain,
            CNLabelPhoneNumberHomeFax,
            CNLabelPhoneNumberWorkFax,
            CNLabelPhoneNumberOtherFax,
            CNLabelPhoneNumberPager,
        ]
    }
}

fn label_name(raw: &str) -> String {
    raw.strip_prefix("_$!<")
        .and_then(|inner| inner.strip_suffix(">!$_"))
        .map_or_else(|| raw.to_owned(), str::to_lowercase)
}

fn label_value(name: &str) -> Retained<NSString> {
    let wanted = name.to_lowercase();
    known_labels()
        .into_iter()
        .find(|label| label_name(&label.to_string()) == wanted)
        .map_or_else(|| NSString::from_str(name), Message::retain)
}

fn read_label<T: Message + NSCopying + NSSecureCoding>(
    value: &CNLabeledValue<T>,
) -> Option<String> {
    unsafe { value.label() }.map(|label| label_name(&label.to_string()))
}

fn labeled<T: Message + NSCopying + NSSecureCoding>(
    label: Option<&str>,
    value: &T,
) -> Retained<CNLabeledValue<T>> {
    let label = label.map(label_value);
    unsafe { CNLabeledValue::labeledValueWithLabel_value(label.as_deref(), value) }
}

fn labeled_strings(values: &NSArray<CNLabeledValue<NSString>>) -> Vec<Labeled> {
    values
        .iter()
        .map(|value| Labeled {
            label: read_label(&value),
            value: unsafe { value.value() }.to_string(),
        })
        .collect()
}

fn write_strings(values: &[Labeled]) -> Retained<NSArray<CNLabeledValue<NSString>>> {
    let values: Vec<_> = values
        .iter()
        .map(|value| labeled(value.label.as_deref(), &*NSString::from_str(&value.value)))
        .collect();
    NSArray::from_retained_slice(&values)
}

fn describe_contact(contact: &CNContact) -> Contact {
    let phones = unsafe { contact.phoneNumbers() }
        .iter()
        .map(|value| Labeled {
            label: read_label(&value),
            value: unsafe { value.value().stringValue() }.to_string(),
        })
        .collect();
    let addresses = unsafe { contact.postalAddresses() }
        .iter()
        .map(|value| {
            let address = unsafe { value.value() };
            Address {
                label: read_label(&value),
                street: unsafe { address.street() }.to_string(),
                city: unsafe { address.city() }.to_string(),
                state: unsafe { address.state() }.to_string(),
                postal_code: unsafe { address.postalCode() }.to_string(),
                country: unsafe { address.country() }.to_string(),
            }
        })
        .collect();
    let birthday = unsafe { contact.birthday() }.and_then(|parts| {
        Some(Birthday {
            year: (parts.year() != NSDateComponentUndefined)
                .then(|| i16::try_from(parts.year()).ok())
                .flatten(),
            month: i8::try_from(parts.month()).ok()?,
            day: i8::try_from(parts.day()).ok()?,
        })
    });
    let emails = unsafe { contact.emailAddresses() };
    let urls = unsafe { contact.urlAddresses() };
    Contact {
        id: unsafe { contact.identifier() }.to_string(),
        given_name: unsafe { contact.givenName() }.to_string(),
        middle_name: unsafe { contact.middleName() }.to_string(),
        family_name: unsafe { contact.familyName() }.to_string(),
        nickname: unsafe { contact.nickname() }.to_string(),
        organization: unsafe { contact.organizationName() }.to_string(),
        department: unsafe { contact.departmentName() }.to_string(),
        job_title: unsafe { contact.jobTitle() }.to_string(),
        emails: labeled_strings(&emails),
        phones,
        addresses,
        urls: labeled_strings(&urls),
        birthday,
    }
}

fn apply_contact(contact: &CNMutableContact, fields: ContactFields) {
    let texts: [(Option<String>, SetText); 7] = [
        (fields.given_name, |c, v| unsafe { c.setGivenName(v) }),
        (fields.middle_name, |c, v| unsafe { c.setMiddleName(v) }),
        (fields.family_name, |c, v| unsafe { c.setFamilyName(v) }),
        (fields.nickname, |c, v| unsafe { c.setNickname(v) }),
        (fields.organization, |c, v| unsafe {
            c.setOrganizationName(v);
        }),
        (fields.department, |c, v| unsafe { c.setDepartmentName(v) }),
        (fields.job_title, |c, v| unsafe { c.setJobTitle(v) }),
    ];
    for (value, set) in texts {
        if let Some(value) = value {
            set(contact, &NSString::from_str(&value));
        }
    }
    if let Some(emails) = &fields.emails {
        unsafe { contact.setEmailAddresses(&write_strings(emails)) };
    }
    if let Some(urls) = &fields.urls {
        unsafe { contact.setUrlAddresses(&write_strings(urls)) };
    }
    if let Some(phones) = &fields.phones {
        let values: Vec<_> = phones
            .iter()
            .filter_map(|phone| {
                let number = unsafe {
                    CNPhoneNumber::phoneNumberWithStringValue(&NSString::from_str(&phone.value))
                }?;
                Some(labeled(phone.label.as_deref(), &*number))
            })
            .collect();
        unsafe { contact.setPhoneNumbers(&NSArray::from_retained_slice(&values)) };
    }
    if let Some(addresses) = &fields.addresses {
        let values: Vec<_> = addresses
            .iter()
            .map(|address| {
                let postal = unsafe { CNMutablePostalAddress::new() };
                unsafe { postal.setStreet(&NSString::from_str(&address.street)) };
                unsafe { postal.setCity(&NSString::from_str(&address.city)) };
                unsafe { postal.setState(&NSString::from_str(&address.state)) };
                unsafe { postal.setPostalCode(&NSString::from_str(&address.postal_code)) };
                unsafe { postal.setCountry(&NSString::from_str(&address.country)) };
                let postal: Retained<CNPostalAddress> = postal.into_super();
                labeled(address.label.as_deref(), &*postal)
            })
            .collect();
        unsafe { contact.setPostalAddresses(&NSArray::from_retained_slice(&values)) };
    }
    match fields.birthday {
        Field::Keep => {}
        Field::Clear => unsafe { contact.setBirthday(None) },
        Field::Set(birthday) => {
            let parts = NSDateComponents::new();
            if let Some(year) = birthday.year {
                parts.setYear(isize::from(year));
            }
            parts.setMonth(isize::from(birthday.month));
            parts.setDay(isize::from(birthday.day));
            unsafe { contact.setBirthday(Some(&parts)) };
        }
    }
}

fn phone(query: &str) -> Option<Retained<CNPhoneNumber>> {
    let digits = query.chars().filter(char::is_ascii_digit).count();
    let is_number = query
        .chars()
        .all(|c| c.is_ascii_digit() || " +-().".contains(c));
    if digits < 3 || !is_number {
        return None;
    }
    unsafe { CNPhoneNumber::phoneNumberWithStringValue(&NSString::from_str(query)) }
}

fn failed(err: &NSError) -> Fault {
    Fault::Failed(err.localizedDescription().to_string())
}

#[cfg(test)]
mod tests {
    use super::super::library::Page;
    use super::*;

    #[test]
    fn contact_labels_read_as_plain_words_and_write_back() {
        assert_eq!(label_name("_$!<Mobile>!$_"), "mobile");
        assert_eq!(label_name("Lab"), "Lab");
        assert_eq!(label_value("mobile").to_string(), "_$!<Mobile>!$_");
        assert_eq!(label_value("Lab").to_string(), "Lab");
    }

    #[test]
    fn an_occurrence_id_carries_its_start_in_utc() {
        let at = occurrence_time("20261126T013000Z").unwrap();

        assert_eq!(at.to_string(), "2026-11-26T01:30:00Z");
        assert_eq!(occurrence_time("20261126T013000"), None);
    }

    #[test]
    #[ignore = "writes to the real calendar, reminders and contacts of this Mac"]
    fn live_library_round_trips_events() {
        let mut mac = Mac::new().unwrap();
        let day = |text: &str| Moment::Day(text.parse().unwrap());
        let at = |text: &str| Moment::At(text.parse().unwrap());
        let calendar = mac
            .create_calendar(CalendarFields {
                title: Some("goat test".to_owned()),
                kind: Some(Kind::Events),
                account: None,
                color: Color::parse("#336699"),
            })
            .unwrap();
        let event = mac
            .create_event(EventFields {
                calendar: Some(calendar.id.clone()),
                title: Some("goat check".to_owned()),
                start: Some(at("2031-03-04T01:00:00Z")),
                end: Some(at("2031-03-04T02:00:00Z")),
                rules: Some(vec![Rule::parse("RRULE:FREQ=WEEKLY;COUNT=3").unwrap()]),
                alerts: Some(vec![15]),
                ..EventFields::default()
            })
            .unwrap();
        let trip = mac
            .create_event(EventFields {
                calendar: Some(calendar.id.clone()),
                title: Some("goat trip".to_owned()),
                start: Some(day("2031-03-20")),
                end: Some(day("2031-03-22")),
                ..EventFields::default()
            })
            .unwrap();
        let listed = mac
            .events(&EventQuery {
                from: "2031-03-01T00:00:00Z".parse().unwrap(),
                to: "2031-03-31T00:00:00Z".parse().unwrap(),
                calendars: vec![calendar.id.clone()],
                text: Some("check".to_owned()),
                page: Page { skip: 0, limit: 10 },
            })
            .unwrap();
        let moved = mac
            .update_event(
                &listed.items[1].id,
                EventFields {
                    title: Some("moved".to_owned()),
                    ..EventFields::default()
                },
            )
            .unwrap();
        let removed = mac.delete_calendar(&calendar.id).unwrap();

        assert_eq!(event.alerts, [15]);
        assert_eq!(trip.start, day("2031-03-20"));
        assert_eq!(trip.end, day("2031-03-22"));
        assert_eq!(listed.items.len(), 3);
        assert_eq!(
            listed.items[0].rules[0].to_line(),
            "RRULE:FREQ=WEEKLY;COUNT=3"
        );
        assert_eq!(listed.items[1].series.as_deref(), event.series.as_deref());
        assert_eq!(moved.title, "moved");
        assert_eq!(moved.start, at("2031-03-11T01:00:00Z"));
        assert_eq!(removed.title, "goat test");
    }

    #[test]
    #[ignore = "writes to the real reminders of this Mac"]
    fn live_library_round_trips_a_reminder() {
        let mut mac = Mac::new().unwrap();
        let day = |text: &str| Moment::Day(text.parse().unwrap());
        let reminder = mac
            .create_reminder(ReminderFields {
                title: Some("goat check".to_owned()),
                due: Field::Set(day("2031-03-04")),
                priority: Some(Priority::High),
                ..ReminderFields::default()
            })
            .unwrap();
        let done = mac
            .update_reminder(
                &reminder.id,
                ReminderFields {
                    is_done: Some(true),
                    ..ReminderFields::default()
                },
            )
            .unwrap();
        mac.delete_reminder(&reminder.id).unwrap();

        assert_eq!(reminder.due, Some(day("2031-03-04")));
        assert_eq!(reminder.priority, Priority::High);
        assert!(done.is_done);
    }

    #[test]
    #[ignore = "writes to the real contacts of this Mac"]
    fn live_library_round_trips_a_contact() {
        let mut mac = Mac::new().unwrap();
        let contact = mac
            .create_contact(ContactFields {
                given_name: Some("Goat".to_owned()),
                family_name: Some("Check".to_owned()),
                emails: Some(vec![Labeled {
                    label: Some("work".to_owned()),
                    value: "goat-check@example.com".to_owned(),
                }]),
                birthday: Field::Set(Birthday::parse("--05-01").unwrap()),
                ..ContactFields::default()
            })
            .unwrap();
        let found = mac
            .contacts(Some("goat-check@example.com"), Page { skip: 0, limit: 5 })
            .unwrap();
        let changed = mac
            .update_contact(
                &contact.id,
                ContactFields {
                    job_title: Some("Tester".to_owned()),
                    ..ContactFields::default()
                },
            )
            .unwrap();
        mac.delete_contact(&contact.id).unwrap();

        assert_eq!(found.items.len(), 1);
        assert_eq!(changed.emails, contact.emails);
        assert_eq!(changed.job_title, "Tester");
        assert_eq!(contact.emails[0].label.as_deref(), Some("work"));
        assert_eq!(
            contact.birthday.map(Birthday::to_text).as_deref(),
            Some("--05-01")
        );
    }
}

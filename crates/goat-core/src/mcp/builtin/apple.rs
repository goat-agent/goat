#[cfg(test)]
mod fake;
mod library;
#[cfg(target_os = "macos")]
#[expect(unsafe_code, reason = "EventKit and Contacts are Objective-C")]
mod macos;
mod rule;

use std::sync::Arc;

use jiff::civil::{Date, Time};
use jiff::{Timestamp, ToSpan};
use rmcp::ErrorData;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::{Value, json};

use self::library::{
    Address, Area, Birthday, Calendar, CalendarFields, Color, Contact, ContactFields, Event,
    EventFields, EventQuery, Fault, Field, Kind, Labeled, Library, Listing, Moment, Page, Person,
    Priority, Reminder, ReminderFields, ReminderQuery, Response, Shelf, Status,
};
use self::rule::Rule;
use super::args::Args;
use super::zone::Zone;

const TOOLS: &str = include_str!("apple.json");
const PAGE: u64 = 100;
const MOST_PAGE: u64 = 250;
const CONTACTS: u64 = 10;
const MOST_CONTACTS: u64 = 1000;
const LISTED_HOURS: i64 = 7 * 24;

#[derive(Debug)]
pub(crate) struct Apple {
    shelf: Shelf,
    granted: Vec<Area>,
    home: String,
    tools: Arc<[Tool]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Calendars,
    Events,
    Reminders,
    Contacts,
}

impl Apple {
    pub(crate) async fn new(shelf: Shelf) -> Result<Self, Fault> {
        let granted = shelf.run(Library::granted).await?;
        let home = shelf.run(Library::time_zone).await?;
        let tools = tools()
            .into_iter()
            .filter(|tool| Group::of(&tool.name).is_some_and(|group| group.is_granted(&granted)))
            .collect();
        Ok(Self {
            shelf,
            granted,
            home,
            tools,
        })
    }

    async fn run(&self, name: &str, args: &Args) -> Result<Value, Fault> {
        let Some(group) = Group::of(name) else {
            return Err(no_tool(name));
        };
        if !group.is_granted(&self.granted) {
            let area = group.areas()[0].settings_name();
            return Err(Fault::Denied(format!(
                "goat has no {area} access; allow the terminal goat runs in under System Settings > Privacy & Security > {area} and start a new session"
            )));
        }
        match group {
            Group::Calendars => self.calendars(name, args).await,
            Group::Events => self.events(name, args).await,
            Group::Reminders => self.reminders(name, args).await,
            Group::Contacts => self.contacts(name, args).await,
        }
    }

    async fn calendars(&self, name: &str, args: &Args) -> Result<Value, Fault> {
        let calendar = match name {
            "list_calendars" => {
                let kind = kind(args)?;
                let granted = self.granted.clone();
                let calendars: Vec<Value> = self
                    .on(Library::calendars)
                    .await?
                    .iter()
                    .filter(|calendar| kind.is_none_or(|kind| calendar.kind == kind))
                    .filter(|calendar| granted.contains(&calendar.kind.area()))
                    .map(calendar_json)
                    .collect();
                return Ok(json!({ "calendars": calendars }));
            }
            "create_calendar" => {
                let fields = CalendarFields {
                    title: Some(required(args, "summary")?),
                    kind: Some(kind(args)?.unwrap_or(Kind::Events)),
                    account: args.word("account").map(str::to_owned),
                    color: color(args)?,
                };
                self.on(move |l| l.create_calendar(fields)).await?
            }
            "update_calendar" => {
                let id = required(args, "calendarId")?;
                let fields = CalendarFields {
                    title: args.word("summary").map(str::to_owned),
                    color: color(args)?,
                    ..CalendarFields::default()
                };
                self.on(move |l| l.update_calendar(&id, fields)).await?
            }
            "delete_calendar" => {
                let id = required(args, "calendarId")?;
                self.on(move |l| l.delete_calendar(&id)).await?
            }
            _ => return Err(no_tool(name)),
        };
        Ok(calendar_json(&calendar))
    }

    async fn events(&self, name: &str, args: &Args) -> Result<Value, Fault> {
        let zone = self.zone(args)?;
        let event = match name {
            "list_events" => return self.list_events(args, &zone).await,
            "get_event" => {
                let id = required(args, "eventId")?;
                self.on(move |l| l.event(&id)).await?
            }
            "create_event" => {
                let (start, end) = times(args, &zone)?;
                let fields = EventFields {
                    calendar: args.word("calendarId").map(str::to_owned),
                    title: Some(required(args, "summary")?),
                    start: Some(start.ok_or_else(|| bad("startTime is required"))?),
                    end: Some(end.ok_or_else(|| bad("endTime is required"))?),
                    ..event_fields(args)?
                };
                self.on(move |l| l.create_event(fields)).await?
            }
            "update_event" => {
                let id = required(args, "eventId")?;
                let (start, mut end) = times(args, &zone)?;
                if let Some(start) = start
                    && end.is_none()
                {
                    let lookup = id.clone();
                    let existing = self.on(move |l| l.event(&lookup)).await?;
                    end = Some(keep_length(&zone, &existing, start)?);
                }
                let fields = EventFields {
                    calendar: args.word("destinationCalendarId").map(str::to_owned),
                    title: args.word("summary").map(str::to_owned),
                    start,
                    end,
                    ..event_fields(args)?
                };
                self.on(move |l| l.update_event(&id, fields)).await?
            }
            "delete_event" => {
                let id = required(args, "eventId")?;
                self.on(move |l| l.delete_event(&id)).await?
            }
            _ => return Err(no_tool(name)),
        };
        Ok(event_json(&zone, &event))
    }

    async fn list_events(&self, args: &Args, zone: &Zone) -> Result<Value, Fault> {
        let from = match args.word("startTime") {
            Some(text) => instant(zone, text)?,
            None => Timestamp::now(),
        };
        let to = match args.word("endTime") {
            Some(text) => instant(zone, text)?,
            None => from
                .checked_add(LISTED_HOURS.hours())
                .map_err(|err| Fault::Failed(err.to_string()))?,
        };
        if to < from {
            return Err(bad("endTime is before startTime"));
        }
        let page = page(args)?;
        let query = EventQuery {
            from,
            to,
            calendars: args
                .word("calendarId")
                .map(str::to_owned)
                .into_iter()
                .collect(),
            text: args.word("fullText").map(str::to_owned),
            page,
        };
        let found = self.on(move |l| l.events(&query)).await?;
        let events: Vec<Value> = found
            .items
            .iter()
            .map(|event| event_json(zone, event))
            .collect();
        let mut out = json!({ "events": events, "timeZone": zone.name });
        next_page(&mut out, &found, page);
        Ok(out)
    }

    async fn reminders(&self, name: &str, args: &Args) -> Result<Value, Fault> {
        let zone = self.zone(args)?;
        let reminder = match name {
            "list_reminders" => {
                let page = page(args)?;
                let query = ReminderQuery {
                    lists: args.word("listId").map(str::to_owned).into_iter().collect(),
                    status: status(args)?,
                    from: args
                        .word("startTime")
                        .map(|text| instant(&zone, text))
                        .transpose()?,
                    to: args
                        .word("endTime")
                        .map(|text| instant(&zone, text))
                        .transpose()?,
                    text: args.word("fullText").map(str::to_owned),
                    page,
                };
                let found = self.on(move |l| l.reminders(&query)).await?;
                let reminders: Vec<Value> = found
                    .items
                    .iter()
                    .map(|reminder| reminder_json(&zone, reminder))
                    .collect();
                let mut out = json!({ "reminders": reminders });
                next_page(&mut out, &found, page);
                return Ok(out);
            }
            "create_reminder" => {
                let fields = ReminderFields {
                    title: Some(required(args, "title")?),
                    ..reminder_fields(args, &zone)?
                };
                self.on(move |l| l.create_reminder(fields)).await?
            }
            "update_reminder" => {
                let id = required(args, "reminderId")?;
                let fields = reminder_fields(args, &zone)?;
                self.on(move |l| l.update_reminder(&id, fields)).await?
            }
            "delete_reminder" => {
                let id = required(args, "reminderId")?;
                self.on(move |l| l.delete_reminder(&id)).await?
            }
            _ => return Err(no_tool(name)),
        };
        Ok(reminder_json(&zone, &reminder))
    }

    async fn contacts(&self, name: &str, args: &Args) -> Result<Value, Fault> {
        let contact = match name {
            "search_contacts" => {
                let query = args.word("query").map(str::to_owned);
                let page = Page {
                    skip: 0,
                    limit: limit(args, "maxResults", CONTACTS, MOST_CONTACTS)?,
                };
                let found = self.on(move |l| l.contacts(query.as_deref(), page)).await?;
                let results: Vec<Value> = found.items.iter().map(contact_json).collect();
                return Ok(json!({ "results": results }));
            }
            "get_user_profile" => {
                let me = self.on(Library::me).await?;
                let mut out = json!({ "name": display_name(&me) });
                put(
                    &mut out,
                    "emailAddress",
                    me.emails.first().map(|email| email.value.as_str()),
                );
                return Ok(out);
            }
            "get_contact" => {
                let id = required(args, "contactId")?;
                self.on(move |l| l.contact(&id)).await?
            }
            "create_contact" => {
                let fields = contact_fields(args)?;
                self.on(move |l| l.create_contact(fields)).await?
            }
            "update_contact" => {
                let id = required(args, "contactId")?;
                let fields = contact_fields(args)?;
                self.on(move |l| l.update_contact(&id, fields)).await?
            }
            "delete_contact" => {
                let id = required(args, "contactId")?;
                self.on(move |l| l.delete_contact(&id)).await?
            }
            _ => return Err(no_tool(name)),
        };
        Ok(contact_json(&contact))
    }

    fn zone(&self, args: &Args) -> Result<Zone, Fault> {
        let name = args.word("timeZone").unwrap_or(&self.home);
        Zone::named(name.to_owned()).map_err(|err| bad(&err))
    }

    async fn on<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut (dyn Library + 'static)) -> Result<T, Fault> + Send + 'static,
    ) -> Result<T, Fault> {
        self.shelf.run(work).await?
    }
}

impl ServerHandler for Apple {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("apple", env!("CARGO_PKG_VERSION")))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tools.to_vec())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Args(request.arguments.unwrap_or_default());
        let result = self
            .run(&request.name, &args)
            .await
            .map_or_else(|fault| failure(&fault), CallToolResult::structured);
        Ok(result.into())
    }
}

impl Group {
    fn of(tool: &str) -> Option<Self> {
        match tool {
            "list_calendars" | "create_calendar" | "update_calendar" | "delete_calendar" => {
                Some(Self::Calendars)
            }
            "list_events" | "get_event" | "create_event" | "update_event" | "delete_event" => {
                Some(Self::Events)
            }
            "list_reminders" | "create_reminder" | "update_reminder" | "delete_reminder" => {
                Some(Self::Reminders)
            }
            "search_contacts" | "get_contact" | "get_user_profile" | "create_contact"
            | "update_contact" | "delete_contact" => Some(Self::Contacts),
            _ => None,
        }
    }

    fn areas(self) -> &'static [Area] {
        match self {
            Self::Calendars => &[Area::Calendars, Area::Reminders],
            Self::Events => &[Area::Calendars],
            Self::Reminders => &[Area::Reminders],
            Self::Contacts => &[Area::Contacts],
        }
    }

    fn is_granted(self, granted: &[Area]) -> bool {
        self.areas().iter().any(|area| granted.contains(area))
    }
}

pub(crate) fn tools() -> Vec<Tool> {
    serde_json::from_str(TOOLS).unwrap_or_else(|err| unreachable!("apple.json is valid: {err}"))
}

fn required(args: &Args, key: &str) -> Result<String, Fault> {
    args.text(key).map_err(|err| bad(&err))
}

fn instant(zone: &Zone, text: &str) -> Result<Timestamp, Fault> {
    zone.at(text)
        .map(|at| at.timestamp())
        .map_err(|err| bad(&err))
}

fn moment(zone: &Zone, text: &str) -> Result<Moment, Fault> {
    if text.len() == 10
        && let Ok(day) = text.parse::<Date>()
    {
        return Ok(Moment::Day(day));
    }
    instant(zone, text).map(Moment::At)
}

fn times(args: &Args, zone: &Zone) -> Result<(Option<Moment>, Option<Moment>), Fault> {
    let (start, end) = (args.word("startTime"), args.word("endTime"));
    if args.flag("allDay") {
        let (Some(start), Some(end)) = (start, end) else {
            return Err(bad("allDay needs startTime and endTime"));
        };
        let (first, last) = zone.days(start, end).map_err(|err| bad(&err))?;
        return Ok((Some(Moment::Day(first)), Some(Moment::Day(last))));
    }
    let start = start.map(|text| instant(zone, text)).transpose()?;
    let end = end.map(|text| instant(zone, text)).transpose()?;
    if let (Some(start), Some(end)) = (start, end)
        && end < start
    {
        return Err(bad("endTime is before startTime"));
    }
    Ok((start.map(Moment::At), end.map(Moment::At)))
}

fn keep_length(zone: &Zone, existing: &Event, start: Moment) -> Result<Moment, Fault> {
    let seconds = |moment: Moment| match moment {
        Moment::At(at) => Ok(at.as_second()),
        Moment::Day(day) => zone
            .civil(day.to_datetime(Time::midnight()))
            .map(|at| at.timestamp().as_second())
            .map_err(|err| bad(&err)),
    };
    let length = seconds(existing.end)? - seconds(existing.start)?;
    let Moment::At(start) = start else {
        return Ok(start);
    };
    start
        .checked_add(length.seconds())
        .map(Moment::At)
        .map_err(|err| Fault::Failed(err.to_string()))
}

fn page(args: &Args) -> Result<Page, Fault> {
    let skip = match args.word("pageToken") {
        Some(token) => token
            .parse()
            .map_err(|_| bad(&format!("{token} is not a page token from this server")))?,
        None => 0,
    };
    Ok(Page {
        skip,
        limit: limit(args, "pageSize", PAGE, MOST_PAGE)?,
    })
}

fn limit(args: &Args, key: &str, default: u64, most: u64) -> Result<usize, Fault> {
    usize::try_from(args.page(key, default, most)).map_err(|err| Fault::Failed(err.to_string()))
}

fn next_page<T>(out: &mut Value, found: &Listing<T>, page: Page) {
    if found.is_truncated {
        out["nextPageToken"] = json!((page.skip + page.limit).to_string());
    }
}

fn text_field(args: &Args, key: &str) -> Result<Field<String>, Fault> {
    match args.value(key) {
        None => Ok(Field::Keep),
        Some(Value::String(text)) if text.is_empty() => Ok(Field::Clear),
        Some(Value::String(text)) => Ok(Field::Set(text.clone())),
        Some(_) => Err(bad(&format!("{key} must be a string"))),
    }
}

fn rules(args: &Args) -> Result<Option<Vec<Rule>>, Fault> {
    if args.value("recurrenceData").is_none() {
        return Ok(None);
    }
    args.list("recurrenceData")
        .iter()
        .map(|line| Rule::parse(line).map_err(|err| bad(&err)))
        .collect::<Result<_, _>>()
        .map(Some)
}

fn minutes(
    args: &Args,
    key: &str,
    read: impl Fn(&Value) -> Option<i64>,
) -> Result<Option<Vec<i64>>, Fault> {
    let Some(value) = args.value(key) else {
        return Ok(None);
    };
    value
        .as_array()
        .and_then(|items| items.iter().map(read).collect())
        .map(Some)
        .ok_or_else(|| bad(&format!("{key} must give whole minutes")))
}

fn kind(args: &Args) -> Result<Option<Kind>, Fault> {
    match args.word("kind") {
        None => Ok(None),
        Some("events") => Ok(Some(Kind::Events)),
        Some("reminders") => Ok(Some(Kind::Reminders)),
        Some(other) => Err(bad(&format!(
            "{other} is not a kind; use events or reminders"
        ))),
    }
}

fn color(args: &Args) -> Result<Option<Color>, Fault> {
    args.word("backgroundColor")
        .map(|text| {
            Color::parse(text).ok_or_else(|| bad(&format!("{text} is not a #RRGGBB color")))
        })
        .transpose()
}

fn status(args: &Args) -> Result<Status, Fault> {
    match args.word("status") {
        None | Some("open") => Ok(Status::Open),
        Some("completed") => Ok(Status::Done),
        Some("all") => Ok(Status::All),
        Some(other) => Err(bad(&format!(
            "{other} is not a status; use open, completed or all"
        ))),
    }
}

fn priority(args: &Args) -> Result<Option<Priority>, Fault> {
    match args.word("priority") {
        None => Ok(None),
        Some("none") => Ok(Some(Priority::None)),
        Some("low") => Ok(Some(Priority::Low)),
        Some("medium") => Ok(Some(Priority::Medium)),
        Some("high") => Ok(Some(Priority::High)),
        Some(other) => Err(bad(&format!(
            "{other} is not a priority; use none, low, medium or high"
        ))),
    }
}

fn event_fields(args: &Args) -> Result<EventFields, Fault> {
    Ok(EventFields {
        location: text_field(args, "location")?,
        notes: text_field(args, "description")?,
        url: text_field(args, "url")?,
        alerts: minutes(args, "overrideReminders", |item| item["minutes"].as_i64())?,
        rules: rules(args)?,
        ..EventFields::default()
    })
}

fn reminder_fields(args: &Args, zone: &Zone) -> Result<ReminderFields, Fault> {
    let due = match text_field(args, "due")? {
        Field::Keep => Field::Keep,
        Field::Clear => Field::Clear,
        Field::Set(text) => Field::Set(moment(zone, &text)?),
    };
    Ok(ReminderFields {
        list: args.word("listId").map(str::to_owned),
        title: args.word("title").map(str::to_owned),
        notes: text_field(args, "notes")?,
        url: text_field(args, "url")?,
        due,
        priority: priority(args)?,
        is_done: args.value("completed").and_then(Value::as_bool),
        alerts: minutes(args, "alerts", Value::as_i64)?,
        rules: rules(args)?,
    })
}

fn contact_fields(args: &Args) -> Result<ContactFields, Fault> {
    let text = |key: &str| args.value(key).and_then(Value::as_str).map(str::to_owned);
    let birthday = match text_field(args, "birthday")? {
        Field::Keep => Field::Keep,
        Field::Clear => Field::Clear,
        Field::Set(raw) => Field::Set(Birthday::parse(&raw).ok_or_else(|| {
            bad(&format!(
                "{raw} is not a birthday; use 1990-05-01 or --05-01"
            ))
        })?),
    };
    Ok(ContactFields {
        given_name: text("givenName"),
        middle_name: text("middleName"),
        family_name: text("familyName"),
        nickname: text("nickname"),
        organization: text("organization"),
        department: text("department"),
        job_title: text("jobTitle"),
        emails: labeled(args, "emailAddresses")?,
        phones: labeled(args, "phoneNumbers")?,
        addresses: addresses(args),
        urls: labeled(args, "urls")?,
        birthday,
    })
}

fn labeled(args: &Args, key: &str) -> Result<Option<Vec<Labeled>>, Fault> {
    if args.value(key).is_none() {
        return Ok(None);
    }
    args.objects(key)
        .map(|item| {
            Some(Labeled {
                label: label(item),
                value: item["value"].as_str()?.to_owned(),
            })
        })
        .collect::<Option<_>>()
        .map(Some)
        .ok_or_else(|| bad(&format!("each of {key} needs a value")))
}

fn addresses(args: &Args) -> Option<Vec<Address>> {
    args.value("addresses")?;
    Some(
        args.objects("addresses")
            .map(|item| {
                let part = |key: &str| item[key].as_str().unwrap_or_default().to_owned();
                Address {
                    label: label(item),
                    street: part("streetAddress"),
                    city: part("city"),
                    state: part("region"),
                    postal_code: part("postalCode"),
                    country: part("country"),
                }
            })
            .collect(),
    )
}

fn label(item: &Value) -> Option<String> {
    item["type"]
        .as_str()
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
}

fn moment_json(zone: &Zone, moment: Moment) -> Value {
    match moment {
        Moment::Day(day) => json!({ "date": day.to_string() }),
        Moment::At(at) => zone.instant(at),
    }
}

fn put(out: &mut Value, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        out[key] = json!(value);
    }
}

fn calendar_json(calendar: &Calendar) -> Value {
    let mut out = json!({
        "id": calendar.id,
        "summary": calendar.title,
        "kind": match calendar.kind {
            Kind::Events => "events",
            Kind::Reminders => "reminders",
        },
        "account": calendar.account,
        "accessRole": if calendar.is_writable { "owner" } else { "reader" },
    });
    if let Some(color) = calendar.color {
        out["backgroundColor"] = json!(color.to_hex());
    }
    if calendar.is_default {
        out["primary"] = json!(true);
    }
    out
}

fn event_json(zone: &Zone, event: &Event) -> Value {
    let mut out = json!({
        "id": event.id,
        "calendarId": event.calendar,
        "summary": event.title,
        "start": moment_json(zone, event.start),
        "end": moment_json(zone, event.end),
    });
    put(&mut out, "description", event.notes.as_deref());
    put(&mut out, "location", event.location.as_deref());
    put(&mut out, "url", event.url.as_deref());
    put(&mut out, "recurringEventId", event.series.as_deref());
    if !event.rules.is_empty() {
        out["recurrence"] = json!(event.rules.iter().map(Rule::to_line).collect::<Vec<_>>());
    }
    if let Some(organizer) = &event.organizer {
        out["organizer"] = person_json(organizer);
    }
    if !event.attendees.is_empty() {
        out["attendees"] = json!(event.attendees.iter().map(person_json).collect::<Vec<_>>());
    }
    if !event.alerts.is_empty() {
        let alerts: Vec<Value> = event
            .alerts
            .iter()
            .map(|minutes| json!({ "method": "popup", "minutes": minutes }))
            .collect();
        out["overrideReminders"] = json!(alerts);
    }
    out
}

fn person_json(person: &Person) -> Value {
    let mut out = json!({
        "responseStatus": match person.response {
            Response::NeedsAction => "needsAction",
            Response::Accepted => "accepted",
            Response::Declined => "declined",
            Response::Tentative => "tentative",
        },
    });
    put(&mut out, "email", person.email.as_deref());
    put(&mut out, "displayName", person.name.as_deref());
    if person.is_self {
        out["self"] = json!(true);
    }
    out
}

fn reminder_json(zone: &Zone, reminder: &Reminder) -> Value {
    let mut out = json!({
        "id": reminder.id,
        "listId": reminder.list,
        "title": reminder.title,
        "priority": match reminder.priority {
            Priority::None => "none",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
        },
        "completed": reminder.is_done,
    });
    put(&mut out, "notes", reminder.notes.as_deref());
    put(&mut out, "url", reminder.url.as_deref());
    if let Some(due) = reminder.due {
        out["due"] = moment_json(zone, due);
    }
    if let Some(at) = reminder.completed {
        out["completedTime"] = zone.instant(at);
    }
    if !reminder.alerts.is_empty() {
        out["alerts"] = json!(reminder.alerts);
    }
    if !reminder.rules.is_empty() {
        out["recurrence"] = json!(reminder.rules.iter().map(Rule::to_line).collect::<Vec<_>>());
    }
    out
}

fn display_name(contact: &Contact) -> String {
    let parts: Vec<&str> = [
        contact.given_name.as_str(),
        contact.middle_name.as_str(),
        contact.family_name.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect();
    if !parts.is_empty() {
        return parts.join(" ");
    }
    [contact.organization.as_str(), contact.nickname.as_str()]
        .into_iter()
        .find(|name| !name.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn contact_json(contact: &Contact) -> Value {
    let mut out = json!({ "id": contact.id, "name": display_name(contact) });
    put(
        &mut out,
        "email",
        contact.emails.first().map(|email| email.value.as_str()),
    );
    for (key, value) in [
        ("givenName", &contact.given_name),
        ("middleName", &contact.middle_name),
        ("familyName", &contact.family_name),
        ("nickname", &contact.nickname),
        ("organization", &contact.organization),
        ("department", &contact.department),
        ("jobTitle", &contact.job_title),
    ] {
        put(&mut out, key, Some(value));
    }
    for (key, values) in [
        ("emailAddresses", &contact.emails),
        ("phoneNumbers", &contact.phones),
        ("urls", &contact.urls),
    ] {
        if !values.is_empty() {
            out[key] = json!(values.iter().map(labeled_json).collect::<Vec<_>>());
        }
    }
    if !contact.addresses.is_empty() {
        out["addresses"] = json!(
            contact
                .addresses
                .iter()
                .map(address_json)
                .collect::<Vec<_>>()
        );
    }
    if let Some(birthday) = contact.birthday {
        out["birthday"] = json!(birthday.to_text());
    }
    out
}

fn labeled_json(value: &Labeled) -> Value {
    let mut out = json!({ "value": value.value });
    put(&mut out, "type", value.label.as_deref());
    out
}

fn address_json(address: &Address) -> Value {
    let mut out = json!({});
    put(&mut out, "type", address.label.as_deref());
    put(&mut out, "streetAddress", Some(&address.street));
    put(&mut out, "city", Some(&address.city));
    put(&mut out, "region", Some(&address.state));
    put(&mut out, "postalCode", Some(&address.postal_code));
    put(&mut out, "country", Some(&address.country));
    out
}

fn no_tool(name: &str) -> Fault {
    Fault::Unsupported(format!("apple has no tool named {name}"))
}

fn bad(message: &str) -> Fault {
    Fault::Failed(format!("bad arguments: {message}"))
}

fn failure(fault: &Fault) -> CallToolResult {
    let next = match fault {
        Fault::Missing(_) => " List again for current ids.",
        Fault::Denied(_) | Fault::Unsupported(_) | Fault::Failed(_) => "",
    };
    CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {}.{next}",
        fault.code(),
        fault.message()
    ))])
}

#[cfg(target_os = "macos")]
pub(crate) fn start() -> Result<Shelf, Fault> {
    Shelf::start("goat-apple", || {
        macos::Mac::new().map(|mac| Box::new(mac) as Box<dyn Library>)
    })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn start() -> Result<Shelf, Fault> {
    Err(unsupported())
}

#[cfg(target_os = "macos")]
pub fn missing_permissions(ask: bool) -> Result<Vec<&'static str>, String> {
    Ok(macos::missing(ask))
}

#[cfg(not(target_os = "macos"))]
pub fn missing_permissions(_ask: bool) -> Result<Vec<&'static str>, String> {
    Err(unsupported().message().to_owned())
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> Fault {
    Fault::Unsupported("Apple's apps are available on macOS only".to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rmcp::service::{
        ClientLifecycleMode, ClientServiceExt, RoleClient, RunningService, ServiceExt,
    };

    use super::fake::Fake;
    use super::*;

    const EVERYTHING: &[Area] = &[Area::Calendars, Area::Reminders, Area::Contacts];
    const SEOUL: &str = "Asia/Seoul";

    struct Rig {
        client: RunningService<RoleClient, ()>,
        _server: RunningService<RoleServer, Apple>,
        log: Arc<Mutex<Vec<String>>>,
    }

    async fn rig(granted: &[Area]) -> Rig {
        let fake = Fake::new(granted);
        let log = Arc::clone(&fake.log);
        let shelf = Shelf::start("goat-apple", move || Ok::<_, Fault>(Box::new(fake))).unwrap();
        let apple = Apple::new(shelf).await.unwrap();
        let (near, far) = tokio::io::duplex(64 * 1024);
        let (server, client) = tokio::join!(
            apple.serve(far),
            ().serve_with_lifecycle(near, ClientLifecycleMode::Initialize)
        );
        Rig {
            client: client.unwrap(),
            _server: server.unwrap(),
            log,
        }
    }

    impl Rig {
        async fn call(&self, name: &str, mut args: Value) -> Result<Value, String> {
            if args.get("timeZone").is_none() {
                args["timeZone"] = json!(SEOUL);
            }
            let mut params = CallToolRequestParams::new(name.to_owned());
            params.arguments = args.as_object().cloned();
            let result = self.client.peer().call_tool(params).await.unwrap();
            let value = serde_json::to_value(&result).unwrap();
            if value["isError"] == true {
                return Err(value["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned());
            }
            Ok(value["structuredContent"].clone())
        }

        async fn tool_names(&self) -> Vec<String> {
            let listed = self.client.peer().list_all_tools().await.unwrap();
            listed
                .into_iter()
                .map(|tool| tool.name.to_string())
                .collect()
        }

        fn log(&self) -> Vec<String> {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[tokio::test]
    async fn tools_of_an_app_without_permission_are_hidden_and_refused() {
        let rig = rig(&[Area::Calendars]).await;

        let names = rig.tool_names().await;
        let err = rig
            .call("search_contacts", json!({ "query": "Kim" }))
            .await
            .unwrap_err();

        assert_eq!(names.len(), 9);
        assert!(names.contains(&"list_events".to_owned()));
        assert!(!names.iter().any(|name| name.contains("reminder")));
        assert!(
            err.starts_with("denied: goat has no Contacts access"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn reminder_lists_are_hidden_without_reminders_access() {
        let rig = rig(&[Area::Calendars]).await;

        let listed = rig.call("list_calendars", json!({})).await.unwrap();

        assert_eq!(
            listed["calendars"][0],
            json!({
                "id": "c1",
                "summary": "Home",
                "kind": "events",
                "account": "iCloud",
                "accessRole": "owner",
                "primary": true,
            })
        );
        assert_eq!(listed["calendars"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_event_is_read_and_shown_in_the_time_zone_it_is_given() {
        let rig = rig(EVERYTHING).await;

        let created = rig
            .call(
                "create_event",
                json!({
                    "summary": "Standup",
                    "startTime": "2026-11-26T09:30:00",
                    "endTime": "2026-11-26T10:00:00",
                    "overrideReminders": [{ "method": "popup", "minutes": 10 }],
                }),
            )
            .await
            .unwrap();
        let in_utc = rig
            .call(
                "get_event",
                json!({ "eventId": created["id"], "timeZone": "UTC" }),
            )
            .await
            .unwrap();

        assert_eq!(
            created["start"],
            json!({ "dateTime": "2026-11-26T09:30:00", "timeZone": SEOUL })
        );
        assert_eq!(created["calendarId"], "c1");
        assert_eq!(
            created["overrideReminders"],
            json!([{ "method": "popup", "minutes": 10 }])
        );
        assert_eq!(in_utc["start"]["dateTime"], "2026-11-26T00:30:00");
    }

    #[tokio::test]
    async fn an_all_day_event_on_one_date_ends_the_next_day() {
        let rig = rig(EVERYTHING).await;

        let created = rig
            .call(
                "create_event",
                json!({
                    "summary": "Trip",
                    "startTime": "2026-11-27",
                    "endTime": "2026-11-27",
                    "allDay": true,
                }),
            )
            .await
            .unwrap();

        assert_eq!(created["start"], json!({ "date": "2026-11-27" }));
        assert_eq!(created["end"], json!({ "date": "2026-11-28" }));
    }

    #[tokio::test]
    async fn an_end_before_the_start_is_refused_before_anything_runs() {
        let rig = rig(EVERYTHING).await;

        let err = rig
            .call(
                "create_event",
                json!({
                    "summary": "x",
                    "startTime": "2026-11-26T10:00:00",
                    "endTime": "2026-11-26T09:00:00",
                }),
            )
            .await
            .unwrap_err();
        let listed = rig
            .call(
                "list_events",
                json!({ "startTime": "2026-11-01", "endTime": "2026-12-01" }),
            )
            .await
            .unwrap();

        assert_eq!(err, "failed: bad arguments: endTime is before startTime.");
        assert_eq!(listed["events"], json!([]));
    }

    #[tokio::test]
    async fn moving_only_the_start_keeps_the_length_and_the_other_fields() {
        let rig = rig(EVERYTHING).await;
        let created = rig
            .call(
                "create_event",
                json!({
                    "summary": "Review",
                    "startTime": "2026-11-26T14:00:00",
                    "endTime": "2026-11-26T15:30:00",
                    "location": "Room 4",
                    "description": "bring notes",
                    "recurrenceData": ["RRULE:FREQ=WEEKLY;COUNT=4"],
                }),
            )
            .await
            .unwrap();

        let updated = rig
            .call(
                "update_event",
                json!({
                    "eventId": created["id"],
                    "startTime": "2026-11-26T16:00:00",
                    "description": "",
                }),
            )
            .await
            .unwrap();

        assert_eq!(updated["summary"], "Review");
        assert_eq!(updated["location"], "Room 4");
        assert_eq!(updated.get("description"), None);
        assert_eq!(updated["end"]["dateTime"], "2026-11-26T17:30:00");
        assert_eq!(updated["recurrence"], json!(["RRULE:FREQ=WEEKLY;COUNT=4"]));
    }

    #[tokio::test]
    async fn recurrence_lines_apple_cannot_store_are_refused() {
        let rig = rig(EVERYTHING).await;

        let err = rig
            .call(
                "create_event",
                json!({
                    "summary": "x",
                    "startTime": "2026-11-26T09:00:00",
                    "endTime": "2026-11-26T10:00:00",
                    "recurrenceData": ["RRULE:FREQ=HOURLY"],
                }),
            )
            .await
            .unwrap_err();

        assert!(err.contains("FREQ=HOURLY is not supported"), "{err}");
    }

    #[tokio::test]
    async fn events_come_in_pages_with_a_token_for_the_next() {
        let rig = rig(EVERYTHING).await;
        for day in ["2026-11-24", "2026-11-25", "2026-11-26"] {
            rig.call(
                "create_event",
                json!({ "summary": day, "startTime": day, "endTime": day, "allDay": true }),
            )
            .await
            .unwrap();
        }
        let range = json!({ "startTime": "2026-11-01", "endTime": "2026-11-30", "pageSize": 2 });

        let first = rig.call("list_events", range.clone()).await.unwrap();
        let mut rest = range;
        rest["pageToken"] = first["nextPageToken"].clone();
        let second = rig.call("list_events", rest).await.unwrap();

        assert_eq!(first["events"][0]["summary"], "2026-11-24");
        assert_eq!(first["nextPageToken"], "2");
        assert_eq!(first["timeZone"], SEOUL);
        assert_eq!(second["events"][0]["summary"], "2026-11-26");
        assert_eq!(second.get("nextPageToken"), None);
    }

    #[tokio::test]
    async fn reminders_list_open_ones_unless_asked_for_more() {
        let rig = rig(EVERYTHING).await;
        let milk = rig
            .call(
                "create_reminder",
                json!({ "title": "Milk", "due": "2026-11-26", "priority": "high" }),
            )
            .await
            .unwrap();
        rig.call(
            "create_reminder",
            json!({ "title": "Bread", "due": "2026-11-26T18:00:00" }),
        )
        .await
        .unwrap();
        rig.call(
            "update_reminder",
            json!({ "reminderId": milk["id"], "completed": true }),
        )
        .await
        .unwrap();

        let open = rig.call("list_reminders", json!({})).await.unwrap();
        let all = rig
            .call("list_reminders", json!({ "status": "all" }))
            .await
            .unwrap();

        assert_eq!(milk["due"], json!({ "date": "2026-11-26" }));
        assert_eq!(milk["priority"], "high");
        assert_eq!(milk["listId"], "l1");
        assert_eq!(open["reminders"][0]["title"], "Bread");
        assert_eq!(
            open["reminders"][0]["due"],
            json!({ "dateTime": "2026-11-26T18:00:00", "timeZone": SEOUL })
        );
        assert_eq!(open["reminders"].as_array().unwrap().len(), 1);
        assert_eq!(all["reminders"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_contact_update_replaces_only_the_lists_it_is_given() {
        let rig = rig(EVERYTHING).await;
        let created = rig
            .call(
                "create_contact",
                json!({
                    "givenName": "Mina",
                    "familyName": "Kim",
                    "emailAddresses": [{ "value": "mina@work.example", "type": "work" }],
                    "phoneNumbers": [{ "value": "010-1234-5678", "type": "mobile" }],
                    "birthday": "--05-01",
                }),
            )
            .await
            .unwrap();

        let updated = rig
            .call(
                "update_contact",
                json!({
                    "contactId": created["id"],
                    "emailAddresses": [{ "value": "mina@home.example" }],
                }),
            )
            .await
            .unwrap();
        let found = rig
            .call("search_contacts", json!({ "query": "mina" }))
            .await
            .unwrap();
        let me = rig.call("get_user_profile", json!({})).await.unwrap();

        assert_eq!(created["name"], "Mina Kim");
        assert_eq!(created["birthday"], "--05-01");
        assert_eq!(
            updated["emailAddresses"],
            json!([{ "value": "mina@home.example" }])
        );
        assert_eq!(updated["phoneNumbers"], created["phoneNumbers"]);
        assert_eq!(found["results"][0]["email"], "mina@home.example");
        assert_eq!(
            me,
            json!({ "name": "Mina Kim", "emailAddress": "mina@home.example" })
        );
    }

    #[tokio::test]
    async fn a_deleted_event_is_returned_as_it_was() {
        let rig = rig(EVERYTHING).await;
        let created = rig
            .call(
                "create_event",
                json!({ "summary": "x", "startTime": "2026-11-26", "endTime": "2026-11-26", "allDay": true }),
            )
            .await
            .unwrap();

        let deleted = rig
            .call("delete_event", json!({ "eventId": created["id"] }))
            .await
            .unwrap();

        assert_eq!(deleted, created);
        assert_eq!(rig.log(), ["delete_event e1"]);
    }

    #[test]
    fn only_the_deleting_tools_are_destructive() {
        let destructive: Vec<String> = tools()
            .into_iter()
            .filter(|tool| {
                tool.annotations
                    .as_ref()
                    .and_then(|annotations| annotations.destructive_hint)
                    == Some(true)
            })
            .map(|tool| tool.name.to_string())
            .collect();

        assert_eq!(
            destructive,
            [
                "delete_calendar",
                "delete_event",
                "delete_reminder",
                "delete_contact"
            ]
        );
    }

    #[test]
    fn every_tool_belongs_to_an_app() {
        for tool in tools() {
            assert!(Group::of(&tool.name).is_some(), "{}", tool.name);
        }
    }

    #[test]
    fn parameters_shared_with_google_have_the_same_names_and_types() {
        let google: Vec<Tool> = [super::super::calendar::TOOLS, super::super::contacts::TOOLS]
            .into_iter()
            .flat_map(|text| serde_json::from_str::<Vec<Tool>>(text).unwrap())
            .collect();
        let mut shared = 0;
        for tool in tools() {
            let Some(twin) = google.iter().find(|twin| twin.name == tool.name) else {
                continue;
            };
            let ours = serde_json::to_value(&tool.input_schema).unwrap();
            let theirs = serde_json::to_value(&twin.input_schema).unwrap();
            for (name, schema) in ours["properties"].as_object().unwrap() {
                let Some(other) = theirs["properties"].get(name) else {
                    continue;
                };
                shared += 1;
                assert_eq!(schema["type"], other["type"], "{} {name}", tool.name);
            }
        }
        assert!(shared >= 25, "{shared}");
    }
}

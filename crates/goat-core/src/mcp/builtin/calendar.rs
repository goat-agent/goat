use jiff::civil::{Time, Weekday};
use jiff::{Timestamp, ToSpan};
use reqwest::{Method, Url};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::args::Args;
use super::google::{self, Rest, copy, items, rename};
use super::zone::Zone;
use crate::oauth;

pub(super) const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
    "https://www.googleapis.com/auth/calendar.events.freebusy",
    "https://www.googleapis.com/auth/calendar.events.readonly",
    "https://www.googleapis.com/auth/calendar.events",
];
pub(super) const TOOLS: &str = include_str!("calendar.json");
const PRIMARY: &str = "primary";
const PAGE: u64 = 100;
const MOST_PAGE: u64 = 250;
const SLOTS: u64 = 5;
const MOST_SLOTS: u64 = 50;
const MINUTES: i64 = 30;
const WORK_START: &str = "09:00";
const WORK_END: &str = "18:00";
const LISTED_DAYS: i64 = 7;
const MEET: &str = "https://meet.google.com/";
const EVENT_TYPES: &[(&str, &str)] = &[
    ("DEFAULT", "default"),
    ("OUT_OF_OFFICE", "outOfOffice"),
    ("FOCUS_TIME", "focusTime"),
    ("WORKING_LOCATION", "workingLocation"),
    ("BIRTHDAY", "birthday"),
    ("FROM_GMAIL", "fromGmail"),
];
const LISTED_TYPES: &[&str] = &["default", "outOfOffice", "focusTime", "fromGmail"];
const PLACES: &[(&str, &str)] = &[
    ("HOME_OFFICE", "homeOffice"),
    ("OFFICE_LOCATION", "officeLocation"),
    ("CUSTOM_LOCATION", "customLocation"),
];
const PERMISSIONS: &[(&str, &str)] = &[
    ("guestsCanInviteOthers", "guestsCanInviteOthers"),
    ("guestsCanModify", "guestsCanModify"),
    ("guestsCanSeeGuests", "guestsCanSeeOtherGuests"),
];
const CODES: &[(&str, &str)] = &[
    ("accessCode", "ACCESS_CODE"),
    ("passcode", "PASSCODE"),
    ("password", "PASSWORD"),
    ("meetingCode", "MEETING_CODE"),
    ("pin", "PIN"),
];

#[derive(Debug)]
pub(super) struct Calendar {
    rest: Rest,
    base: Box<str>,
    home: OnceCell<String>,
}

struct Search {
    from: Timestamp,
    to: Timestamp,
    day_start: Time,
    day_end: Time,
    skips_weekends: bool,
    seconds: i64,
    wanted: usize,
}

impl Calendar {
    pub(super) fn new(rest: Rest, base: &str) -> Self {
        Self {
            rest,
            base: base.into(),
            home: OnceCell::new(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "list_events" => self.list_events(args).await,
            "search_events" => self.search_events(args).await,
            "get_event" => {
                let found = self.event_of(args).await?;
                Ok(event(&found))
            }
            "list_calendars" => self.list_calendars(args).await,
            "suggest_time" => self.suggest_time(args).await,
            "create_event" => self.create_event(args).await,
            "update_event" => self.update_event(args).await,
            "delete_event" => self.delete_event(args).await,
            "respond_to_event" => self.respond_to_event(args).await,
            _ => Err(google::no_tool(name)),
        }
    }

    async fn list_events(&self, args: &Args) -> Result<Value, String> {
        let zone = self.zone(args).await?;
        let start = match args.word("startTime") {
            Some(text) => zone.at(text)?,
            None => Timestamp::now().to_zoned_datetime(zone.tz.clone()),
        };
        let end = match args.word("endTime") {
            Some(text) => zone.at(text)?,
            None => start
                .checked_add(LISTED_DAYS.days())
                .map_err(|err| err.to_string())?,
        };
        let mut query = vec![
            ("singleEvents", "true".to_owned()),
            ("timeMin", start.timestamp().to_string()),
            ("timeMax", end.timestamp().to_string()),
            (
                "maxResults",
                args.page("pageSize", PAGE, MOST_PAGE).to_string(),
            ),
            ("timeZone", zone.name.clone()),
        ];
        if let Some(token) = args.word("pageToken") {
            query.push(("pageToken", token.to_owned()));
        }
        if let Some(text) = args.word("fullText") {
            query.push(("q", text.to_owned()));
        }
        let order = args.word("orderBy");
        match order {
            Some("startTime" | "startTimeDesc") => query.push(("orderBy", "startTime".to_owned())),
            Some("lastModified") => query.push(("orderBy", "updated".to_owned())),
            _ => {}
        }
        let mut wanted = args.list("eventType");
        if wanted.is_empty() {
            wanted = args.list("eventTypeFilter");
        }
        let types: Vec<&str> = if wanted.is_empty() {
            LISTED_TYPES.to_vec()
        } else {
            wanted.iter().filter_map(|kind| event_type(kind)).collect()
        };
        query.extend(
            types
                .into_iter()
                .map(|kind| ("eventTypes", kind.to_owned())),
        );
        let calendar = args.word("calendarId").unwrap_or(PRIMARY);
        let found = self
            .rest
            .get(self.url(&["calendars", calendar, "events"], &query)?)
            .await?;
        let mut events: Vec<Value> = items(&found, "items").map(event).collect();
        if order == Some("startTimeDesc") {
            events.reverse();
        }
        let mut out = json!({ "events": events });
        for field in [
            "summary",
            "description",
            "updated",
            "timeZone",
            "accessRole",
            "defaultReminders",
            "nextPageToken",
        ] {
            copy(&found, &mut out, field);
        }
        Ok(out)
    }

    async fn search_events(&self, args: &Args) -> Result<Value, String> {
        let zone = self.zone(args).await?;
        let mut query = vec![
            ("q", args.text("query")?),
            ("singleEvents", "true".to_owned()),
            (
                "maxResults",
                args.page("pageSize", PAGE, MOST_PAGE).to_string(),
            ),
            ("timeZone", zone.name),
        ];
        if let Some(token) = args.word("pageToken") {
            query.push(("pageToken", token.to_owned()));
        }
        let found = self
            .rest
            .get(self.url(&["calendars", PRIMARY, "events"], &query)?)
            .await?;
        let mut out = json!({ "events": items(&found, "items").map(event).collect::<Vec<_>>() });
        copy(&found, &mut out, "nextPageToken");
        Ok(out)
    }

    async fn list_calendars(&self, args: &Args) -> Result<Value, String> {
        let mut query = vec![(
            "maxResults",
            args.page("pageSize", PAGE, MOST_PAGE).to_string(),
        )];
        if let Some(token) = args.word("pageToken") {
            query.push(("pageToken", token.to_owned()));
        }
        let found = self
            .rest
            .get(self.url(&["users", "me", "calendarList"], &query)?)
            .await?;
        let calendars: Vec<Value> = items(&found, "items")
            .map(|entry| {
                let mut out = json!({ "id": entry["id"] });
                let title = entry
                    .get("summaryOverride")
                    .filter(|title| title.is_string())
                    .unwrap_or(&entry["summary"]);
                if !title.is_null() {
                    out["summary"] = title.clone();
                }
                copy(entry, &mut out, "description");
                copy(entry, &mut out, "timeZone");
                out
            })
            .collect();
        let mut out = json!({ "calendars": calendars });
        copy(&found, &mut out, "nextPageToken");
        Ok(out)
    }

    async fn suggest_time(&self, args: &Args) -> Result<Value, String> {
        let zone = self.zone(args).await?;
        let from = zone.at(&args.text("startTime")?)?.timestamp();
        let to = zone.at(&args.text("endTime")?)?.timestamp();
        if to <= from {
            return Err("endTime must be after startTime".to_owned());
        }
        let emails = args.list("attendeeEmails");
        if emails.is_empty() {
            return Err("attendeeEmails is required".to_owned());
        }
        let preferences = args.value("preferences").cloned().unwrap_or_default();
        let hour = |field: &str, default: &str| {
            let text = preferences[field].as_str().unwrap_or(default);
            text.parse::<Time>()
                .map_err(|_| format!("{field} {text} is not an HH:mm time"))
        };
        let search = Search {
            from,
            to,
            day_start: hour("startHour", WORK_START)?,
            day_end: hour("endHour", WORK_END)?,
            skips_weekends: preferences["excludeWeekends"].as_bool() == Some(true),
            seconds: args
                .value("durationMinutes")
                .and_then(Value::as_i64)
                .filter(|minutes| *minutes > 0)
                .unwrap_or(MINUTES)
                * 60,
            wanted: usize::try_from(
                preferences["pageSize"]
                    .as_u64()
                    .filter(|size| *size > 0)
                    .unwrap_or(SLOTS)
                    .min(MOST_SLOTS),
            )
            .unwrap_or(usize::MAX),
        };
        let found = self
            .rest
            .send(
                Method::POST,
                self.url(&["freeBusy"], &[])?,
                &json!({
                    "timeMin": from.to_string(),
                    "timeMax": to.to_string(),
                    "timeZone": zone.name,
                    "items": emails.iter().map(|email| json!({ "id": email })).collect::<Vec<_>>(),
                }),
            )
            .await?;
        let mut busy = Vec::new();
        let mut unread = Vec::new();
        for email in &emails {
            let calendar = &found["calendars"][email];
            if items(calendar, "errors").next().is_some() {
                unread.push(email.clone());
            }
            for period in items(calendar, "busy") {
                if let (Some(start), Some(end)) = (
                    period["start"]
                        .as_str()
                        .and_then(|text| text.parse::<Timestamp>().ok()),
                    period["end"]
                        .as_str()
                        .and_then(|text| text.parse::<Timestamp>().ok()),
                ) {
                    busy.push((start, end));
                }
            }
        }
        busy.sort();
        let mut out = json!({ "timeSlots": free_slots(&zone, &search, &busy)? });
        if !unread.is_empty() {
            out["unavailableCalendars"] = json!(unread);
        }
        Ok(out)
    }

    async fn create_event(&self, args: &Args) -> Result<Value, String> {
        let zone = self.zone(args).await?;
        let (start, end) = zone.span(
            &args.text("startTime")?,
            &args.text("endTime")?,
            args.flag("allDay"),
        )?;
        let mut body = json!({ "summary": args.text("summary")?, "start": start, "end": end });
        let mut guests: Vec<Value> = args.objects("attendees").map(guest_to_rest).collect();
        for email in args.list("attendeeEmails") {
            add_guest(&mut guests, json!({ "email": email }));
        }
        if !guests.is_empty() {
            body["attendees"] = json!(guests);
        }
        let recurrence = args.list("recurrenceData");
        if !recurrence.is_empty() {
            body["recurrence"] = json!(recurrence);
        }
        let attachments: Vec<Value> = args.objects("attachments").map(attachment).collect();
        if !attachments.is_empty() {
            body["attachments"] = json!(attachments);
        }
        if let Some(kind) = args.word("eventType").and_then(event_type) {
            body["eventType"] = Value::String(kind.to_owned());
        }
        if let Some(place) = args.value("workingLocationProperties") {
            body["workingLocationProperties"] = place_to_rest(place);
        }
        describe(args, &mut body);
        let calendar = args.word("calendarId").unwrap_or(PRIMARY);
        let created = self
            .rest
            .send(
                Method::POST,
                self.url(&["calendars", calendar, "events"], &writing(args, "all"))?,
                &body,
            )
            .await?;
        Ok(event(&created))
    }

    async fn update_event(&self, args: &Args) -> Result<Value, String> {
        let zone = self.zone(args).await?;
        let existing = self.event_of(args).await?;
        let mut patch = json!({});
        if let Some(summary) = args.word("summary") {
            patch["summary"] = Value::String(summary.to_owned());
        }
        describe(args, &mut patch);
        let (start, end) = (args.word("startTime"), args.word("endTime"));
        if args.flag("allDay") {
            let (Some(start), Some(end)) = (start, end) else {
                return Err("allDay needs startTime and endTime".to_owned());
            };
            let (start, end) = zone.span(start, end, true)?;
            patch["start"] = start;
            patch["end"] = end;
        } else if let Some(start) = start {
            let start = zone.at(start)?;
            let end = if let Some(end) = end {
                zone.at(end)?
            } else {
                let length = zone.length(&existing)?;
                start
                    .checked_add(length.seconds())
                    .map_err(|err| err.to_string())?
            };
            patch["start"] = zone.moment(&start);
            patch["end"] = zone.moment(&end);
        } else if let Some(end) = end {
            patch["end"] = zone.moment(&zone.at(end)?);
        }
        let removed: Vec<String> = args
            .list("removedAttendeeEmails")
            .iter()
            .map(|email| email.to_lowercase())
            .collect();
        let added: Vec<Value> = args
            .objects("addedAttendees")
            .map(guest_to_rest)
            .chain(
                args.list("addedAttendeeEmails")
                    .into_iter()
                    .map(|email| json!({ "email": email })),
            )
            .collect();
        if !removed.is_empty() || !added.is_empty() {
            let mut guests: Vec<Value> = items(&existing, "attendees")
                .filter(|found| {
                    let email = found["email"].as_str().unwrap_or_default().to_lowercase();
                    !removed.contains(&email)
                })
                .cloned()
                .collect();
            for new in added {
                add_guest(&mut guests, new);
            }
            patch["attendees"] = json!(guests);
        }
        let dropped = args.list("removedAttachmentFileUrls");
        let attached: Vec<Value> = args.objects("addedAttachments").map(attachment).collect();
        if !dropped.is_empty() || !attached.is_empty() {
            let mut kept: Vec<Value> = items(&existing, "attachments")
                .filter(|found| {
                    !dropped
                        .iter()
                        .any(|url| found["fileUrl"].as_str() == Some(url.as_str()))
                })
                .cloned()
                .collect();
            kept.extend(attached);
            patch["attachments"] = json!(kept);
        }
        let updated = self.patch(args, &patch, "all").await?;
        Ok(event(&updated))
    }

    async fn delete_event(&self, args: &Args) -> Result<Value, String> {
        let existing = self.event_of(args).await?;
        let calendar = args.word("calendarId").unwrap_or(PRIMARY);
        let id = args.text("eventId")?;
        self.rest
            .delete(self.url(
                &["calendars", calendar, "events", &id],
                &[("sendUpdates", notification(args, "all").to_owned())],
            )?)
            .await?;
        Ok(event(&existing))
    }

    async fn respond_to_event(&self, args: &Args) -> Result<Value, String> {
        let status = args.text("responseStatus")?;
        let existing = self.event_of(args).await?;
        let mut guests: Vec<Value> = items(&existing, "attendees").cloned().collect();
        let me = guests
            .iter_mut()
            .find(|guest| guest["self"].as_bool() == Some(true))
            .ok_or("the user is not a guest of this event")?;
        me["responseStatus"] = Value::String(status);
        if let Some(comment) = args.word("responseComment") {
            me["comment"] = Value::String(comment.to_owned());
        }
        let updated = self
            .patch(args, &json!({ "attendees": guests }), "externalOnly")
            .await?;
        Ok(event(&updated))
    }

    async fn event_of(&self, args: &Args) -> Result<Value, String> {
        let calendar = args.word("calendarId").unwrap_or(PRIMARY);
        let id = args.text("eventId")?;
        self.rest
            .get(self.url(&["calendars", calendar, "events", &id], &[])?)
            .await
    }

    async fn patch(&self, args: &Args, patch: &Value, notify: &str) -> Result<Value, String> {
        let calendar = args.word("calendarId").unwrap_or(PRIMARY);
        let id = args.text("eventId")?;
        self.rest
            .send(
                Method::PATCH,
                self.url(
                    &["calendars", calendar, "events", &id],
                    &writing(args, notify),
                )?,
                patch,
            )
            .await
    }

    async fn zone(&self, args: &Args) -> Result<Zone, String> {
        let name = match args.word("timeZone") {
            Some(name) => name.to_owned(),
            None => self
                .home
                .get_or_try_init(|| async {
                    let found = self
                        .rest
                        .get(self.url(&["users", "me", "calendarList", PRIMARY], &[])?)
                        .await?;
                    Ok::<_, String>(found["timeZone"].as_str().unwrap_or("UTC").to_owned())
                })
                .await?
                .clone(),
        };
        Zone::named(name)
    }

    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Url, String> {
        let path: Vec<&str> = ["calendar", "v3"]
            .into_iter()
            .chain(segments.iter().copied())
            .collect();
        google::at(&self.base, &path, query)
    }
}

fn free_slots(
    zone: &Zone,
    search: &Search,
    busy: &[(Timestamp, Timestamp)],
) -> Result<Vec<Value>, String> {
    let mut slots = Vec::new();
    let last = search.to.to_zoned_datetime(zone.tz.clone()).date();
    let mut day = search.from.to_zoned_datetime(zone.tz.clone()).date();
    while day <= last && slots.len() < search.wanted {
        let is_weekend = matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday);
        if !(search.skips_weekends && is_weekend) {
            let open = zone
                .civil(day.to_datetime(search.day_start))?
                .timestamp()
                .max(search.from);
            let close = zone
                .civil(day.to_datetime(search.day_end))?
                .timestamp()
                .min(search.to);
            let mut cursor = open;
            for &(start, end) in busy {
                if end <= cursor || start >= close {
                    continue;
                }
                if start.as_second() - cursor.as_second() >= search.seconds {
                    slots.push(zone.slot(cursor, start));
                }
                cursor = cursor.max(end);
            }
            if close.as_second() - cursor.as_second() >= search.seconds {
                slots.push(zone.slot(cursor, close));
            }
        }
        day = day.tomorrow().map_err(|err| err.to_string())?;
    }
    slots.truncate(search.wanted);
    Ok(slots)
}

fn writing(args: &Args, notify: &str) -> Vec<(&'static str, String)> {
    vec![
        ("sendUpdates", notification(args, notify).to_owned()),
        ("conferenceDataVersion", "1".to_owned()),
        ("supportsAttachments", "true".to_owned()),
    ]
}

fn notification<'a>(args: &Args, default: &'a str) -> &'a str {
    match args.word("notificationLevel") {
        Some("NONE") => "none",
        Some("EXTERNAL_ONLY") => "externalOnly",
        Some("ALL") => "all",
        _ => default,
    }
}

fn describe(args: &Args, body: &mut Value) {
    for field in ["description", "location", "colorId", "visibility"] {
        if let Some(value) = args.word(field) {
            body[field] = Value::String(value.to_owned());
        }
    }
    match args.word("availability") {
        Some("AVAILABILITY_BUSY") => body["transparency"] = json!("opaque"),
        Some("AVAILABILITY_FREE") => body["transparency"] = json!("transparent"),
        _ => {}
    }
    if let Some(permissions) = args.value("guestPermissions") {
        for (official, rest) in PERMISSIONS {
            rename(permissions, official, body, rest);
        }
    }
    let overrides: Vec<Value> = args
        .objects("overrideReminders")
        .map(|reminder| json!({ "method": reminder["method"], "minutes": reminder["minutes"] }))
        .collect();
    if !overrides.is_empty() {
        body["reminders"] = json!({ "useDefault": false, "overrides": overrides });
    } else if args.has("useDefaultReminders") {
        body["reminders"] = json!({ "useDefault": args.flag("useDefaultReminders") });
    }
    if let Some(meeting) = args.word("googleMeetUrl") {
        let uri = if meeting.starts_with("http") {
            meeting.to_owned()
        } else {
            format!("{MEET}{meeting}")
        };
        body["conferenceData"] = json!({
            "conferenceSolution": { "key": { "type": "hangoutsMeet" } },
            "entryPoints": [{ "entryPointType": "video", "uri": uri }],
        });
    } else if args.flag("addGoogleMeetUrl") {
        body["conferenceData"] = json!({
            "createRequest": {
                "requestId": oauth::random_token::<16>(),
                "conferenceSolutionKey": { "type": "hangoutsMeet" },
            },
        });
    }
}

fn guest_to_rest(found: &Value) -> Value {
    let mut out = json!({ "email": found["email"] });
    for field in [
        "displayName",
        "resource",
        "responseStatus",
        "additionalGuests",
    ] {
        copy(found, &mut out, field);
    }
    rename(found, "optionalAttendee", &mut out, "optional");
    out
}

fn add_guest(guests: &mut Vec<Value>, new: Value) {
    let email = new["email"].as_str().unwrap_or_default().to_lowercase();
    let is_known = guests
        .iter()
        .any(|guest| guest["email"].as_str().unwrap_or_default().to_lowercase() == email);
    if !is_known {
        guests.push(new);
    }
}

fn attachment(found: &Value) -> Value {
    let mut out = json!({ "fileUrl": found["fileUrl"] });
    copy(found, &mut out, "title");
    out
}

fn event_type(official: &str) -> Option<&'static str> {
    if official == "EVENT_TYPE_UNSPECIFIED" {
        return Some("default");
    }
    EVENT_TYPES
        .iter()
        .find(|(name, _)| *name == official)
        .map(|(_, rest)| *rest)
}

fn place_to_rest(found: &Value) -> Value {
    let kind = PLACES
        .iter()
        .find(|(name, _)| found["type"].as_str() == Some(name))
        .map(|(_, rest)| *rest);
    let mut out = json!({ "type": kind });
    match kind {
        Some("homeOffice") => out["homeOffice"] = json!({}),
        Some("customLocation") => {
            out["customLocation"] = json!({ "label": found["customLocationLabel"] });
        }
        Some("officeLocation") => out["officeLocation"] = found["officeLocation"].clone(),
        _ => {}
    }
    out
}

fn event(found: &Value) -> Value {
    let mut out = json!({});
    for field in [
        "id",
        "status",
        "htmlLink",
        "created",
        "updated",
        "summary",
        "description",
        "location",
        "start",
        "end",
        "recurrence",
        "recurringEventId",
        "originalStartTime",
        "colorId",
        "visibility",
    ] {
        copy(found, &mut out, field);
    }
    for field in ["creator", "organizer"] {
        if let Some(person) = found.get(field).filter(|person| person.is_object()) {
            out[field] = picked(person, &["email", "displayName", "self"]);
        }
    }
    let guests: Vec<Value> = items(found, "attendees").map(guest_from_rest).collect();
    if !guests.is_empty() {
        out["attendees"] = json!(guests);
    }
    let attachments: Vec<Value> = items(found, "attachments").map(attachment).collect();
    if !attachments.is_empty() {
        out["attachments"] = json!(attachments);
    }
    let reminders = &found["reminders"];
    rename(reminders, "useDefault", &mut out, "useDefaultReminders");
    rename(reminders, "overrides", &mut out, "overrideReminders");
    if PERMISSIONS
        .iter()
        .any(|(_, rest)| found.get(rest).is_some())
    {
        let mut permissions = json!({});
        for (official, rest) in PERMISSIONS {
            rename(found, rest, &mut permissions, official);
        }
        out["guestPermissions"] = permissions;
    }
    out["availability"] = json!(if found["transparency"].as_str() == Some("transparent") {
        "AVAILABILITY_FREE"
    } else {
        "AVAILABILITY_BUSY"
    });
    let kind = found["eventType"].as_str().unwrap_or("default");
    if let Some((official, _)) = EVENT_TYPES.iter().find(|(_, rest)| *rest == kind) {
        out["eventType"] = json!(official);
    }
    if let Some(place) = found
        .get("workingLocationProperties")
        .filter(|place| place.is_object())
    {
        out["workingLocationProperties"] = place_from_rest(place);
    }
    if let Some(conference) = conference(found) {
        out["conferenceData"] = conference;
    }
    out
}

fn picked(found: &Value, fields: &[&str]) -> Value {
    let mut out = json!({});
    for field in fields {
        copy(found, &mut out, field);
    }
    out
}

fn guest_from_rest(found: &Value) -> Value {
    let mut out = picked(
        found,
        &[
            "email",
            "displayName",
            "resource",
            "responseStatus",
            "comment",
            "organizer",
            "self",
            "additionalGuests",
            "id",
        ],
    );
    rename(found, "optional", &mut out, "optionalAttendee");
    out
}

fn place_from_rest(place: &Value) -> Value {
    let mut out = json!({});
    if let Some((official, _)) = PLACES
        .iter()
        .find(|(_, rest)| place["type"].as_str() == Some(rest))
    {
        out["type"] = json!(official);
    }
    rename(
        &place["customLocation"],
        "label",
        &mut out,
        "customLocationLabel",
    );
    copy(place, &mut out, "officeLocation");
    out
}

fn conference(found: &Value) -> Option<Value> {
    let data = found
        .get("conferenceData")
        .filter(|data| data.is_object())?;
    let mut out = json!({});
    rename(
        &data["conferenceSolution"],
        "name",
        &mut out,
        "solutionName",
    );
    rename(data, "notes", &mut out, "joiningNotesText");
    let mut phones = Vec::new();
    for point in items(data, "entryPoints") {
        let codes: Vec<Value> = CODES
            .iter()
            .filter_map(|(field, kind)| {
                point[*field]
                    .as_str()
                    .map(|value| json!({ "codeType": kind, "value": value }))
            })
            .collect();
        let mut entry = json!({ "uri": point["uri"] });
        if !codes.is_empty() {
            entry["codes"] = json!(codes);
        }
        match point["entryPointType"].as_str() {
            Some("video") => out["videoEntryPoint"] = entry,
            Some("sip") => out["sipEntryPoint"] = entry,
            Some("more") => out["moreDetailsEntryPoint"] = entry,
            Some("phone") => {
                let number = point["label"].as_str().map_or_else(
                    || {
                        point["uri"]
                            .as_str()
                            .unwrap_or_default()
                            .trim_start_matches("tel:")
                            .to_owned()
                    },
                    str::to_owned,
                );
                let mut phone = json!({ "phoneNumber": number });
                if !codes.is_empty() {
                    phone["codes"] = json!(codes);
                }
                copy(point, &mut phone, "regionCode");
                phones.push(phone);
            }
            _ => {}
        }
    }
    if !phones.is_empty() {
        out["phoneEntryPoints"] = json!(phones);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::google::Product;
    use super::super::google::fake::{self, serve};

    const SEOUL: &str = "GET /calendar/v3/users/me/calendarList/primary";

    fn seoul() -> (&'static str, fake::Reply) {
        (
            SEOUL,
            fake::json(&json!({ "id": "me@example.com", "timeZone": "Asia/Seoul" })),
        )
    }

    #[tokio::test]
    async fn a_local_time_is_read_in_the_calendars_own_zone() {
        let google = serve(vec![
            seoul(),
            (
                "GET /calendar/v3/calendars/primary/events",
                fake::json(&json!({ "summary": "Mino", "timeZone": "Asia/Seoul", "items": [] })),
            ),
        ])
        .await;

        let listed = google
            .call(
                Product::Calendar,
                "list_events",
                json!({ "startTime": "2026-04-30T10:00:00", "endTime": "2026-04-30T18:00:00" }),
            )
            .await
            .unwrap();

        let seen = google.one("GET", "/calendar/v3/calendars/primary/events");
        assert_eq!(seen.query("timeMin"), ["2026-04-30T01:00:00Z"]);
        assert_eq!(seen.query("timeMax"), ["2026-04-30T09:00:00Z"]);
        assert_eq!(seen.query("timeZone"), ["Asia/Seoul"]);
        assert_eq!(
            seen.query("eventTypes"),
            ["default", "outOfOffice", "focusTime", "fromGmail"]
        );
        assert_eq!(
            listed,
            json!({ "events": [], "summary": "Mino", "timeZone": "Asia/Seoul" })
        );
    }

    #[tokio::test]
    async fn a_named_zone_follows_its_daylight_saving_time() {
        let google = serve(vec![(
            "GET /calendar/v3/calendars/primary/events",
            fake::json(&json!({ "items": [] })),
        )])
        .await;

        google
            .call(
                Product::Calendar,
                "list_events",
                json!({ "startTime": "2026-07-01T09:00:00", "endTime": "2026-12-01T09:00:00", "timeZone": "America/New_York" }),
            )
            .await
            .unwrap();

        let seen = google.one("GET", "/calendar/v3/calendars/primary/events");
        assert_eq!(seen.query("timeMin"), ["2026-07-01T13:00:00Z"]);
        assert_eq!(seen.query("timeMax"), ["2026-12-01T14:00:00Z"]);
        assert!(
            google
                .seen("GET", "/calendar/v3/users/me/calendarList/primary")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn an_event_is_shaped_like_the_official_servers() {
        let google = serve(vec![(
            "GET /calendar/v3/calendars/primary/events/e1",
            fake::json(&json!({
                "id": "e1",
                "summary": "회의",
                "start": { "dateTime": "2026-04-30T10:00:00+09:00", "timeZone": "Asia/Seoul" },
                "end": { "dateTime": "2026-04-30T11:00:00+09:00", "timeZone": "Asia/Seoul" },
                "transparency": "transparent",
                "eventType": "outOfOffice",
                "attendees": [{ "email": "kim@example.com", "optional": true, "responseStatus": "accepted" }],
                "reminders": { "useDefault": false, "overrides": [{ "method": "popup", "minutes": 10 }] },
                "guestsCanModify": true,
                "conferenceData": {
                    "conferenceSolution": { "name": "Google Meet" },
                    "entryPoints": [
                        { "entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij" },
                        { "entryPointType": "phone", "uri": "tel:+1-555-0100", "label": "+1 555-0100", "pin": "1234", "regionCode": "US" },
                    ],
                },
            })),
        )])
        .await;

        let found = google
            .call(Product::Calendar, "get_event", json!({ "eventId": "e1" }))
            .await
            .unwrap();

        assert_eq!(
            found,
            json!({
                "id": "e1",
                "summary": "회의",
                "start": { "dateTime": "2026-04-30T10:00:00+09:00", "timeZone": "Asia/Seoul" },
                "end": { "dateTime": "2026-04-30T11:00:00+09:00", "timeZone": "Asia/Seoul" },
                "attendees": [{ "email": "kim@example.com", "responseStatus": "accepted", "optionalAttendee": true }],
                "useDefaultReminders": false,
                "overrideReminders": [{ "method": "popup", "minutes": 10 }],
                "guestPermissions": { "guestsCanModify": true },
                "availability": "AVAILABILITY_FREE",
                "eventType": "OUT_OF_OFFICE",
                "conferenceData": {
                    "solutionName": "Google Meet",
                    "videoEntryPoint": { "uri": "https://meet.google.com/abc-defg-hij" },
                    "phoneEntryPoints": [{
                        "phoneNumber": "+1 555-0100",
                        "codes": [{ "codeType": "PIN", "value": "1234" }],
                        "regionCode": "US",
                    }],
                },
            })
        );
    }

    #[tokio::test]
    async fn a_new_event_is_sent_in_local_time_with_its_guests_and_a_meet_link() {
        let google = serve(vec![
            seoul(),
            (
                "POST /calendar/v3/calendars/primary/events",
                fake::json(&json!({ "id": "e9" })),
            ),
        ])
        .await;

        google
            .call(
                Product::Calendar,
                "create_event",
                json!({
                    "summary": "회의",
                    "startTime": "2026-04-30T10:00:00",
                    "endTime": "2026-04-30T11:00:00",
                    "attendees": [{ "email": "kim@example.com", "optionalAttendee": true }],
                    "attendeeEmails": ["KIM@example.com", "lee@example.com"],
                    "addGoogleMeetUrl": true,
                    "availability": "AVAILABILITY_FREE",
                    "notificationLevel": "NONE",
                }),
            )
            .await
            .unwrap();

        let seen = google.one("POST", "/calendar/v3/calendars/primary/events");
        let body = seen.json();
        assert_eq!(seen.query("sendUpdates"), ["none"]);
        assert_eq!(seen.query("conferenceDataVersion"), ["1"]);
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-04-30T10:00:00", "timeZone": "Asia/Seoul" })
        );
        assert_eq!(
            body["attendees"],
            json!([{ "email": "kim@example.com", "optional": true }, { "email": "lee@example.com" }])
        );
        assert_eq!(body["transparency"], "transparent");
        assert_eq!(
            body["conferenceData"]["createRequest"]["conferenceSolutionKey"]["type"],
            "hangoutsMeet"
        );
    }

    #[tokio::test]
    async fn an_all_day_event_on_one_date_ends_the_next_day() {
        let google = serve(vec![
            seoul(),
            (
                "POST /calendar/v3/calendars/primary/events",
                fake::json(&json!({ "id": "e9" })),
            ),
        ])
        .await;

        google
            .call(
                Product::Calendar,
                "create_event",
                json!({ "summary": "휴가", "startTime": "2026-04-30", "endTime": "2026-04-30", "allDay": true }),
            )
            .await
            .unwrap();

        let body = google
            .one("POST", "/calendar/v3/calendars/primary/events")
            .json();
        assert_eq!(body["start"], json!({ "date": "2026-04-30" }));
        assert_eq!(body["end"], json!({ "date": "2026-05-01" }));
    }

    #[tokio::test]
    async fn moving_only_the_start_keeps_the_events_length_and_other_guests() {
        let google = serve(vec![
            seoul(),
            (
                "GET /calendar/v3/calendars/primary/events/e1",
                fake::json(&json!({
                    "id": "e1",
                    "start": { "dateTime": "2026-04-30T10:00:00+09:00" },
                    "end": { "dateTime": "2026-04-30T11:30:00+09:00" },
                    "attendees": [
                        { "email": "kim@example.com", "responseStatus": "accepted" },
                        { "email": "lee@example.com" },
                    ],
                })),
            ),
            (
                "PATCH /calendar/v3/calendars/primary/events/e1",
                fake::json(&json!({ "id": "e1" })),
            ),
        ])
        .await;

        google
            .call(
                Product::Calendar,
                "update_event",
                json!({
                    "eventId": "e1",
                    "startTime": "2026-04-30T14:00:00",
                    "removedAttendeeEmails": ["LEE@example.com"],
                    "addedAttendeeEmails": ["park@example.com"],
                }),
            )
            .await
            .unwrap();

        let patch = google
            .one("PATCH", "/calendar/v3/calendars/primary/events/e1")
            .json();
        assert_eq!(
            patch["end"],
            json!({ "dateTime": "2026-04-30T15:30:00", "timeZone": "Asia/Seoul" })
        );
        assert_eq!(
            patch["attendees"],
            json!([
                { "email": "kim@example.com", "responseStatus": "accepted" },
                { "email": "park@example.com" },
            ])
        );
    }

    #[tokio::test]
    async fn a_response_changes_only_the_users_own_answer() {
        let google = serve(vec![
            (
                "GET /calendar/v3/calendars/primary/events/e1",
                fake::json(&json!({
                    "id": "e1",
                    "attendees": [
                        { "email": "boss@example.com", "organizer": true, "responseStatus": "accepted" },
                        { "email": "me@example.com", "self": true, "responseStatus": "needsAction" },
                    ],
                })),
            ),
            (
                "PATCH /calendar/v3/calendars/primary/events/e1",
                fake::json(&json!({ "id": "e1" })),
            ),
        ])
        .await;

        google
            .call(
                Product::Calendar,
                "respond_to_event",
                json!({ "eventId": "e1", "responseStatus": "declined", "responseComment": "출장" }),
            )
            .await
            .unwrap();

        let seen = google.one("PATCH", "/calendar/v3/calendars/primary/events/e1");
        assert_eq!(seen.query("sendUpdates"), ["externalOnly"]);
        assert_eq!(
            seen.json()["attendees"],
            json!([
                { "email": "boss@example.com", "organizer": true, "responseStatus": "accepted" },
                { "email": "me@example.com", "self": true, "responseStatus": "declined", "comment": "출장" },
            ])
        );
    }

    #[tokio::test]
    async fn free_time_skips_busy_hours_weekends_and_the_night() {
        let google = serve(vec![
            seoul(),
            (
                "POST /calendar/v3/freeBusy",
                fake::json(&json!({ "calendars": {
                    "me@example.com": { "busy": [{ "start": "2026-11-27T01:00:00Z", "end": "2026-11-27T03:00:00Z" }] },
                    "kim@other.com": { "errors": [{ "reason": "notFound" }] },
                }})),
            ),
        ])
        .await;

        let suggested = google
            .call(
                Product::Calendar,
                "suggest_time",
                json!({
                    "attendeeEmails": ["me@example.com", "kim@other.com"],
                    "startTime": "2026-11-27T00:00:00",
                    "endTime": "2026-11-30T23:00:00",
                    "durationMinutes": 60,
                    "preferences": { "excludeWeekends": true },
                }),
            )
            .await
            .unwrap();

        let slot = |start: &str, end: &str| {
            json!({
                "start": { "dateTime": start, "timeZone": "Asia/Seoul" },
                "end": { "dateTime": end, "timeZone": "Asia/Seoul" },
            })
        };
        assert_eq!(
            suggested,
            json!({
                "timeSlots": [
                    slot("2026-11-27T09:00:00", "2026-11-27T10:00:00"),
                    slot("2026-11-27T12:00:00", "2026-11-27T18:00:00"),
                    slot("2026-11-30T09:00:00", "2026-11-30T18:00:00"),
                ],
                "unavailableCalendars": ["kim@other.com"],
            })
        );
    }

    #[tokio::test]
    async fn a_deleted_event_is_returned_as_it_was() {
        let google = serve(vec![
            (
                "GET /calendar/v3/calendars/primary/events/e1",
                fake::json(&json!({ "id": "e1", "summary": "회의" })),
            ),
            (
                "DELETE /calendar/v3/calendars/primary/events/e1",
                fake::status(204, &json!({})),
            ),
        ])
        .await;

        let deleted = google
            .call(
                Product::Calendar,
                "delete_event",
                json!({ "eventId": "e1" }),
            )
            .await
            .unwrap();

        assert_eq!(
            google
                .one("DELETE", "/calendar/v3/calendars/primary/events/e1")
                .query("sendUpdates"),
            ["all"]
        );
        assert_eq!(deleted["summary"], "회의");
    }
}

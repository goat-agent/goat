---
description: How to use the calendars, reminders and contacts on this Mac with mcp.apple. Load it before the first apple call in a session.
user-invocable: false
---
`mcp.apple` reaches Calendar, Reminders and Contacts on this Mac, with every account the Mac syncs: iCloud, Google, Exchange and others. Its event and contact tools take the same arguments and return the same shapes as Google's calendar and contacts servers.

Times:
- Pass local times without an offset, such as `2026-11-26T09:30:00`. They are read in this Mac's time zone unless you pass `timeZone`.
- Work out relative dates such as "next Tuesday" from today's date before you call.
- An all-day event takes `allDay: true`, and its `endTime` is the day after its last day.

Calendars and lists:
- `list_calendars()` gives the ids of calendars and reminder lists. `primary` marks the default of each kind; use it unless the user names another.

Events:
- `list_events({ startTime, endTime })` lists each occurrence of a repeating event with its own `id` and the series' `recurringEventId`.
- An occurrence's `id` changes or deletes that occurrence only; the `recurringEventId` changes or deletes the whole series.
- Apple cannot invite guests. When the user wants invitations or a meeting link, use a Google calendar server if one is connected. The Mac's calendar also shows Google calendars, so prefer the Google server for events in a Google account.

Reminders:
- `list_reminders()` shows open reminders; pass `status: "completed"` or `"all"` for others.
- `due` takes a date for a whole day or a time. `update_reminder({ reminderId, completed: true })` completes one.

Contacts:
- `search_contacts({ query })` takes a name, an email address or a phone number. When it returns several people, ask which one the user means.
- A list given to `update_contact`, such as `emailAddresses`, replaces the whole list. Read the contact with `get_contact` first and send the full list with your change.

Deleting asks the user first. When the user refuses, stop and say what you wanted to delete.

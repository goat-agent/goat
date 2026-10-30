import { tool } from "ai";
import { z } from "zod";
import { callGoogle, GoogleConnection, googleScopes } from "./google";

const API = "https://www.googleapis.com/calendar/v3/calendars/primary/events";
const PAGE = 50;
const metadata = { scope: googleScopes.calendar };

const When = z.object({ dateTime: z.string().optional(), date: z.string().optional() });

const Event = z.object({
  id: z.string(),
  summary: z.string().optional(),
  start: When.optional(),
  end: When.optional(),
  location: z.string().optional(),
  htmlLink: z.string().optional(),
  attendees: z.array(z.object({ email: z.string().optional() })).optional(),
});

const Events = z.object({ items: z.array(Event).default([]) });

const time = z
  .string()
  .describe("RFC 3339 date-time with offset (2026-10-31T15:00:00+09:00), or a date for all-day");

export const googleCalendarTools = {
  google_calendar_list_events: tool({
    description: "Google Calendar: list events on the user's primary calendar in a time range.",
    inputSchema: z.object({
      timeMin: z.string().describe("Start of the range, RFC 3339 with offset"),
      timeMax: z.string().describe("End of the range, RFC 3339 with offset"),
      query: z.string().optional().describe("Words to match in the events"),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ timeMin, timeMax, query }, { context, abortSignal }) => {
      const url = new URL(API);
      url.search = new URLSearchParams({
        timeMin,
        timeMax,
        singleEvents: "true",
        orderBy: "startTime",
        maxResults: String(PAGE),
        ...(query ? { q: query } : {}),
      }).toString();
      const { items } = Events.parse(await callGoogle(context.fetch, url, { signal: abortSignal }));
      return { events: items.map(publicEvent) };
    },
  }),
  google_calendar_create_event: tool({
    description: "Google Calendar: create an event on the user's primary calendar.",
    inputSchema: z.object({
      title: z.string(),
      start: time,
      end: time,
      location: z.string().optional(),
      description: z.string().optional(),
      attendees: z.array(z.email()).optional().describe("Guests to invite"),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async (input, { context, abortSignal }) => {
      const body = eventBody(input);
      const event = Event.parse(
        await callGoogle(context.fetch, API, { method: "POST", body, signal: abortSignal }),
      );
      return publicEvent(event);
    },
  }),
  google_calendar_update_event: tool({
    description: "Google Calendar: change an event's title, time, place, notes, or guests.",
    inputSchema: z.object({
      eventId: z.string(),
      title: z.string().optional(),
      start: time.optional(),
      end: time.optional(),
      location: z.string().optional(),
      description: z.string().optional(),
      attendees: z.array(z.email()).optional().describe("The full guest list after the change"),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ eventId, ...changes }, { context, abortSignal }) => {
      const url = `${API}/${encodeURIComponent(eventId)}`;
      const body = eventBody(changes);
      const event = Event.parse(
        await callGoogle(context.fetch, url, { method: "PATCH", body, signal: abortSignal }),
      );
      return publicEvent(event);
    },
  }),
  google_calendar_delete_event: tool({
    description: "Google Calendar: delete an event; guests are told it was cancelled.",
    inputSchema: z.object({ eventId: z.string() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ eventId }, { context, abortSignal }) => {
      const url = `${API}/${encodeURIComponent(eventId)}?sendUpdates=all`;
      await callGoogle(context.fetch, url, { method: "DELETE", signal: abortSignal });
      return { deleted: eventId };
    },
  }),
};

function eventBody(input: {
  title?: string | undefined;
  start?: string | undefined;
  end?: string | undefined;
  location?: string | undefined;
  description?: string | undefined;
  attendees?: string[] | undefined;
}) {
  return {
    ...(input.title === undefined ? {} : { summary: input.title }),
    ...(input.start === undefined ? {} : { start: when(input.start) }),
    ...(input.end === undefined ? {} : { end: when(input.end) }),
    ...(input.location === undefined ? {} : { location: input.location }),
    ...(input.description === undefined ? {} : { description: input.description }),
    ...(input.attendees === undefined
      ? {}
      : { attendees: input.attendees.map((email) => ({ email })) }),
  };
}

function when(value: string) {
  return /^\d{4}-\d{2}-\d{2}$/.test(value) ? { date: value } : { dateTime: value };
}

function publicEvent(event: z.infer<typeof Event>) {
  return {
    id: event.id,
    title: event.summary ?? "",
    start: event.start?.dateTime ?? event.start?.date,
    end: event.end?.dateTime ?? event.end?.date,
    location: event.location,
    attendees: event.attendees?.flatMap((attendee) => attendee.email ?? []),
    link: event.htmlLink,
  };
}

import { tool } from "ai";
import { z } from "zod";
import { callGraph, MicrosoftConnection, untrusted, UNTRUSTED_NOTE } from "./microsoft";

const API = "https://graph.microsoft.com/v1.0/me";
const FIELDS = "id,subject,start,end,isAllDay,location,attendees,bodyPreview,webLink";
const PAGE = 50;

const When = z.object({ dateTime: z.string(), timeZone: z.string().optional() });

const Event = z.object({
  id: z.string(),
  subject: z.string().nullish(),
  start: When.optional(),
  end: When.optional(),
  isAllDay: z.boolean().optional(),
  location: z.object({ displayName: z.string().optional() }).optional(),
  attendees: z
    .array(z.object({ emailAddress: z.object({ address: z.string().optional() }) }))
    .optional(),
  bodyPreview: z.string().optional(),
  webLink: z.string().optional(),
});

const Events = z.object({ value: z.array(Event).default([]) });

const timeZone = z
  .string()
  .describe("IANA time zone the times are in, such as Asia/Seoul; the time tool shows it");
const time = z
  .string()
  .describe("Local date-time without offset (2026-10-31T15:00:00), or a date for all-day");

type Changes = {
  title?: string | undefined;
  start?: string | undefined;
  end?: string | undefined;
  timeZone?: string | undefined;
  location?: string | undefined;
  description?: string | undefined;
  attendees?: string[] | undefined;
};

const readMetadata = { effect: "read" };

export const outlookCalendarTools = {
  outlook_calendar_list_events: tool({
    description: `Outlook Calendar: list events on the user's default calendar in a time range, with times in the given time zone. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({
      start: z.string().describe("Start of the range, ISO 8601 with offset"),
      end: z.string().describe("End of the range, ISO 8601 with offset"),
      timeZone,
    }),
    contextSchema: MicrosoftConnection,
    metadata: readMetadata,
    execute: async ({ start, end, timeZone: zone }, { context, abortSignal }) => {
      const url = new URL(`${API}/calendarView`);
      url.search = new URLSearchParams({
        startDateTime: start,
        endDateTime: end,
        $top: String(PAGE),
        $orderby: "start/dateTime",
        $select: FIELDS,
      }).toString();
      const headers = { prefer: `outlook.timezone="${zone}"` };
      const { value } = Events.parse(
        await callGraph(context.fetch, url, { headers, signal: abortSignal }),
      );
      return { events: value.map(publicEvent) };
    },
  }),
  outlook_calendar_create_event: tool({
    description: "Outlook Calendar: create an event on the user's default calendar.",
    inputSchema: z.object({
      title: z.string(),
      start: time,
      end: time,
      timeZone,
      location: z.string().optional(),
      description: z.string().optional(),
      attendees: z.array(z.email()).optional().describe("Guests to invite"),
    }),
    contextSchema: MicrosoftConnection,
    execute: async (input, { context, abortSignal }) => {
      const event = Event.parse(
        await callGraph(context.fetch, `${API}/events`, {
          method: "POST",
          body: eventBody(input),
          signal: abortSignal,
        }),
      );
      return publicEvent(event);
    },
  }),
  outlook_calendar_update_event: tool({
    description:
      "Outlook Calendar: change an event's title, time, place, notes, or guests. Pass timeZone when changing start or end.",
    inputSchema: z.object({
      eventId: z.string(),
      title: z.string().optional(),
      start: time.optional(),
      end: time.optional(),
      timeZone: timeZone.optional(),
      location: z.string().optional(),
      description: z.string().optional(),
      attendees: z.array(z.email()).optional().describe("The full guest list after the change"),
    }),
    contextSchema: MicrosoftConnection,
    execute: async ({ eventId, ...changes }, { context, abortSignal }) => {
      const event = Event.parse(
        await callGraph(context.fetch, `${API}/events/${encodeURIComponent(eventId)}`, {
          method: "PATCH",
          body: eventBody(changes),
          signal: abortSignal,
        }),
      );
      return publicEvent(event);
    },
  }),
  outlook_calendar_delete_event: tool({
    description:
      "Outlook Calendar: delete an event; guests are told it was cancelled if the user organized it.",
    inputSchema: z.object({ eventId: z.string() }),
    contextSchema: MicrosoftConnection,
    execute: async ({ eventId }, { context, abortSignal }) => {
      await callGraph(context.fetch, `${API}/events/${encodeURIComponent(eventId)}`, {
        method: "DELETE",
        signal: abortSignal,
      });
      return { deleted: eventId };
    },
  }),
};

function eventBody(input: Changes) {
  const moment = (value: string) => {
    if (input.timeZone === undefined) throw new Error("timeZone is required to set a time");
    const dateTime = /^\d{4}-\d{2}-\d{2}$/.test(value) ? `${value}T00:00:00` : value;
    return { dateTime, timeZone: input.timeZone };
  };
  const isAllDay = [input.start, input.end].some((value) => value && !value.includes("T"));
  return {
    ...(input.title === undefined ? {} : { subject: input.title }),
    ...(input.start === undefined ? {} : { start: moment(input.start) }),
    ...(input.end === undefined ? {} : { end: moment(input.end) }),
    ...(isAllDay ? { isAllDay: true } : {}),
    ...(input.location === undefined ? {} : { location: { displayName: input.location } }),
    ...(input.description === undefined
      ? {}
      : { body: { contentType: "Text", content: input.description } }),
    ...(input.attendees === undefined
      ? {}
      : {
          attendees: input.attendees.map((address) => ({
            emailAddress: { address },
            type: "required",
          })),
        }),
  };
}

function publicEvent(event: z.infer<typeof Event>) {
  return {
    id: event.id,
    title: untrusted(event.subject ?? ""),
    start: event.start?.dateTime,
    end: event.end?.dateTime,
    timeZone: event.start?.timeZone,
    isAllDay: event.isAllDay,
    location: event.location?.displayName,
    attendees: event.attendees?.flatMap((attendee) => attendee.emailAddress.address ?? []),
    notes: event.bodyPreview ? untrusted(event.bodyPreview) : undefined,
    link: event.webLink,
  };
}

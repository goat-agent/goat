import { tool } from "ai";
import { z } from "zod";
import {
  createSchedule,
  deleteSchedule,
  LocalTime,
  updateSchedule,
  viewsFor,
} from "../../jobs/schedules";
import { refuseIfTainted } from "../trust";

const ScheduleContext = z.object({ agentId: z.string(), timeZone: z.string() });
const ChangeContext = ScheduleContext.extend({ isTainted: z.boolean() });

const title = z
  .string()
  .min(1)
  .max(60)
  .describe("A short name for the schedule list, in the language the user writes in");
const instruction = z
  .string()
  .min(1)
  .describe(
    "What to do when it runs, as a standalone request to yourself. The run sees no conversation, so include every detail and preference it needs.",
  );
const rrule = z
  .string()
  .describe(
    "For a repeating schedule: an RFC 5545 RRULE without DTSTART, e.g. FREQ=WEEKLY;BYDAY=MO",
  );

// Each tool returns every schedule, so the agent sees the result of its own change.
export const scheduleTools = {
  schedule_create: tool({
    description:
      "Schedule yourself to do something later, once or repeating, such as a reminder or a daily summary. Times are in the user's time zone. When it runs, you act on the instruction and your report appears in this conversation.",
    inputSchema: z.object({
      title,
      instruction,
      start: LocalTime.describe("The first run, YYYY-MM-DDTHH:MM in the user's time zone"),
      rrule: rrule.optional(),
    }),
    contextSchema: ChangeContext,
    execute: async (input, { context }) => {
      refuseIfTainted(context.isTainted, "schedule");
      return { schedules: await createSchedule(context.agentId, context.timeZone, input) };
    },
  }),
  schedule_update: tool({
    description: "Change a schedule's title, instruction, or timing.",
    inputSchema: z.object({
      id: z.string(),
      title: title.optional(),
      instruction: instruction.optional(),
      start: LocalTime.optional(),
      rrule: rrule.nullable().optional().describe("null makes it run once"),
    }),
    contextSchema: ChangeContext,
    execute: async ({ id, ...change }, { context }) => {
      refuseIfTainted(context.isTainted, "schedule");
      return { schedules: await updateSchedule(context.agentId, context.timeZone, id, change) };
    },
  }),
  schedule_cancel: tool({
    description: "Cancel a schedule.",
    inputSchema: z.object({ id: z.string() }),
    contextSchema: ScheduleContext,
    execute: async ({ id }, { context }) => {
      await deleteSchedule(context.agentId, id, { fromAgent: true });
      return { schedules: await viewsFor(context.agentId, context.timeZone) };
    },
  }),
};

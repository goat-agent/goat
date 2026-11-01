import type { LanguageModelUsage } from "ai";
import { sql } from "drizzle-orm";
import {
  bigint,
  boolean,
  index,
  integer,
  jsonb,
  pgTable,
  text,
  timestamp,
  unique,
  uuid,
} from "drizzle-orm/pg-core";
import { agents, messages } from "./agents";

/** Things an agent does later on its own: for now, schedules. */
export const jobs = pgTable(
  "jobs",
  {
    id: uuid()
      .primaryKey()
      .default(sql`uuidv7()`),
    agentId: uuid()
      .notNull()
      .references(() => agents.id, { onDelete: "cascade" }),
    // What the schedule list shows, in the language the user wrote in.
    title: text().notNull(),
    instruction: text().notNull(),
    // Wall-clock time in whatever time zone the user is in, so schedules follow them.
    start: timestamp({ mode: "string" }).notNull(),
    rrule: text(),
    enabled: boolean().notNull().default(true),
    // Derived from start, rrule, and the user's time zone; null once nothing is left to run.
    nextRunAt: timestamp({ withTimezone: true }),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [
    index().on(table.agentId),
    index()
      .on(table.nextRunAt)
      .where(sql`${table.enabled} and ${table.nextRunAt} is not null`),
  ],
);

/** One firing of a job. The unique key makes firing idempotent across server instances. */
export const jobRuns = pgTable(
  "job_runs",
  {
    id: bigint({ mode: "number" }).primaryKey().generatedAlwaysAsIdentity(),
    jobId: uuid()
      .notNull()
      .references(() => jobs.id, { onDelete: "cascade" }),
    scheduledFor: timestamp({ withTimezone: true }).notNull(),
    owner: uuid(),
    leaseUntil: timestamp({ withTimezone: true }),
    attempts: integer().notNull().default(0),
    finishedAt: timestamp({ withTimezone: true }),
    error: text({ enum: ["missed", "failed"] }),
    outcome: text(),
    model: text(),
    usage: jsonb().$type<LanguageModelUsage>(),
    messageId: bigint({ mode: "number" }).references(() => messages.id, { onDelete: "cascade" }),
  },
  (table) => [
    unique().on(table.jobId, table.scheduledFor),
    index()
      .on(table.jobId)
      .where(sql`${table.finishedAt} is null`),
  ],
);

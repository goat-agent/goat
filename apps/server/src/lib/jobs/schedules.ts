import { and, asc, desc, eq, inArray, isNotNull } from "drizzle-orm";
import { z } from "zod";
import { db, type Transaction } from "../../db";
import { agents } from "../../db/schema/agents";
import { jobRuns, jobs } from "../../db/schema/jobs";
import { nextRun, parseRule } from "./recurrence";

export const SCHEDULES_PREFIX = "Schedules:";

export const LocalTime = z
  .string()
  .regex(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/, "Use YYYY-MM-DDTHH:MM")
  .describe("Local date and time in the user's time zone, YYYY-MM-DDTHH:MM");

export class ScheduleNotFound extends Error {}

type Job = typeof jobs.$inferSelect;
type Change = { title?: string; instruction?: string; start?: string; rrule?: string | null };

/** What the agent sees of a schedule; `next` is local time. */
export type ScheduleView = {
  id: string;
  title: string;
  instruction: string;
  start: string;
  rrule: string | null;
  enabled: boolean;
  next: string | null;
};

export async function createSchedule(
  agentId: string,
  timeZone: string,
  input: { title: string; instruction: string; start: string; rrule?: string },
): Promise<ScheduleView[]> {
  const nextRunAt = firstRun(input.start, input.rrule ?? null, timeZone);
  await db.transaction(async (tx) => {
    await tx.insert(jobs).values({
      agentId,
      title: input.title,
      instruction: input.instruction,
      start: input.start,
      rrule: input.rrule ?? null,
      nextRunAt,
    });
    await rememberNote(tx, agentId);
  });
  return viewsFor(agentId, timeZone);
}

export async function updateSchedule(
  agentId: string,
  timeZone: string,
  id: string,
  change: Change,
): Promise<ScheduleView[]> {
  await db.transaction(async (tx) => {
    const job = await lockJob(tx, agentId, id);
    const start = change.start ?? job.start;
    const rrule = change.rrule === undefined ? job.rrule : change.rrule;
    await tx
      .update(jobs)
      .set({
        title: change.title ?? job.title,
        instruction: change.instruction ?? job.instruction,
        start,
        rrule,
        nextRunAt: firstRun(start, rrule, timeZone),
      })
      .where(eq(jobs.id, id));
    await rememberNote(tx, agentId);
  });
  return viewsFor(agentId, timeZone);
}

/** `fromAgent` keeps the agent from being told about a change it made itself. */
export async function deleteSchedule(
  agentId: string,
  id: string,
  { fromAgent }: { fromAgent: boolean },
): Promise<void> {
  await db.transaction(async (tx) => {
    await lockJob(tx, agentId, id);
    await tx.delete(jobs).where(eq(jobs.id, id));
    if (fromAgent) await rememberNote(tx, agentId);
  });
}

export async function setEnabled(
  agentId: string,
  timeZone: string,
  id: string,
  enabled: boolean,
): Promise<void> {
  await db.transaction(async (tx) => {
    const job = await lockJob(tx, agentId, id);
    const nextRunAt = enabled ? nextAfterNow(job, timeZone) : job.nextRunAt;
    await tx.update(jobs).set({ enabled, nextRunAt }).where(eq(jobs.id, id));
  });
}

/** Schedules follow the user, so a new time zone moves every next run. */
export async function moveSchedules(userId: string, timeZone: string): Promise<void> {
  const owned = await db
    .select({ job: jobs })
    .from(jobs)
    .innerJoin(agents, eq(agents.id, jobs.agentId))
    .where(and(eq(agents.userId, userId), isNotNull(jobs.nextRunAt)));
  for (const { job } of owned) {
    await db
      .update(jobs)
      .set({ nextRunAt: nextAfterNow(job, timeZone) })
      .where(eq(jobs.id, job.id));
  }
}

export async function viewsFor(agentId: string, timeZone: string): Promise<ScheduleView[]> {
  const rows = await db
    .select()
    .from(jobs)
    .where(eq(jobs.agentId, agentId))
    .orderBy(asc(jobs.createdAt));
  return rows.map((job) => ({
    id: job.id,
    title: job.title,
    instruction: job.instruction,
    start: wallClock(job.start).toString({ smallestUnit: "minute" }),
    rrule: job.rrule,
    enabled: job.enabled,
    next: job.nextRunAt
      ? job.nextRunAt
          .toTemporalInstant()
          .toZonedDateTimeISO(timeZone)
          .toPlainDateTime()
          .toString({ smallestUnit: "minute" })
      : null,
  }));
}

/** Schedules for the settings screen, each with how its latest finished run went. */
export async function listSchedules(agentId: string) {
  const rows = await db
    .select()
    .from(jobs)
    .where(eq(jobs.agentId, agentId))
    .orderBy(asc(jobs.createdAt));
  const latest = await latestErrors(rows.map((job) => job.id));
  return rows.map((job) => ({ job, error: latest.get(job.id) ?? null }));
}

/** The note the agent gets when its schedules changed without it, or undefined if unchanged. */
export async function scheduleNoteChange(
  tx: Transaction,
  agentId: string,
  previous: string | null,
): Promise<string | undefined> {
  const note = await noteFor(tx, agentId);
  if (note === previous || (previous === null && note === undefined)) return undefined;
  const sent = note ?? `${SCHEDULES_PREFIX} none.`;
  await tx.update(agents).set({ schedulesNote: sent }).where(eq(agents.id, agentId));
  return sent;
}

async function rememberNote(tx: Transaction, agentId: string): Promise<void> {
  const note = (await noteFor(tx, agentId)) ?? `${SCHEDULES_PREFIX} none.`;
  await tx.update(agents).set({ schedulesNote: note }).where(eq(agents.id, agentId));
}

async function noteFor(tx: Transaction, agentId: string): Promise<string | undefined> {
  const rows = await tx
    .select()
    .from(jobs)
    .where(eq(jobs.agentId, agentId))
    .orderBy(asc(jobs.createdAt));
  if (rows.length === 0) return undefined;
  const lines = rows.map((job) => {
    const timing = `start ${wallClock(job.start).toString({ smallestUnit: "minute" })}, ${job.rrule ?? "once"}`;
    return `- ${job.id}: ${job.title} (${timing})${job.enabled ? "" : " (paused)"}`;
  });
  return `${SCHEDULES_PREFIX}\n${lines.join("\n")}`;
}

async function lockJob(tx: Transaction, agentId: string, id: string): Promise<Job> {
  const [job] = await tx
    .select()
    .from(jobs)
    .where(and(eq(jobs.id, id), eq(jobs.agentId, agentId)))
    .for("update");
  if (!job) throw new ScheduleNotFound(`No schedule with id ${id}.`);
  return job;
}

async function latestErrors(jobIds: string[]): Promise<Map<string, "missed" | "failed" | null>> {
  if (jobIds.length === 0) return new Map();
  const runs = await db
    .selectDistinctOn([jobRuns.jobId], { jobId: jobRuns.jobId, error: jobRuns.error })
    .from(jobRuns)
    .where(and(inArray(jobRuns.jobId, jobIds), isNotNull(jobRuns.finishedAt)))
    .orderBy(jobRuns.jobId, desc(jobRuns.scheduledFor));
  return new Map(runs.map((run) => [run.jobId, run.error]));
}

function firstRun(start: string, rrule: string | null, timeZone: string): Date {
  if (rrule) parseRule(rrule);
  const next = nextRun(
    { start: wallClock(start), rrule },
    timeZone,
    Temporal.Now.instant().subtract({ seconds: 1 }),
  );
  if (!next) throw new Error("That time has already passed.");
  return new Date(next.epochMilliseconds);
}

export function nextAfterNow(
  job: { start: string; rrule: string | null },
  timeZone: string,
): Date | null {
  const next = nextRun(
    { start: wallClock(job.start), rrule: job.rrule },
    timeZone,
    Temporal.Now.instant(),
  );
  return next ? new Date(next.epochMilliseconds) : null;
}

// Postgres returns `timestamp without time zone` as "YYYY-MM-DD HH:MM:SS".
function wallClock(start: string): Temporal.PlainDateTime {
  return Temporal.PlainDateTime.from(start.replace(" ", "T")).round({
    smallestUnit: "minute",
  });
}

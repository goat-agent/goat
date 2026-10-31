import { generateText, Output, stepCountIs } from "ai";
import { and, desc, eq, gte, inArray, isNotNull, isNull, lt, lte, ne, or, sql } from "drizzle-orm";
import { z } from "zod";
import { db } from "../../db";
import { agents } from "../../db/schema/agents";
import { users } from "../../db/schema/auth";
import { jobRuns, jobs } from "../../db/schema/jobs";
import { wake } from "../agent";
import { scheduledRunInstructionsFor } from "../ai/instructions";
import { model, providerOptions } from "../ai/providers";
import { historyTools } from "../ai/tools/history";
import { memoryTools } from "../ai/tools/memory";
import { searchTools } from "../ai/tools/search";
import { timeTools } from "../ai/tools/time";
import { webTools } from "../ai/tools/web";
import { dateIn } from "../dates";
import { type Catalog, openCatalog } from "../integrations/catalog";
import { logger } from "../logger";
import * as memory from "../memory";
import { nextAfterNow } from "./schedules";

const LEASE = sql`interval '60 seconds'`;
// A morning summary at noon helps nobody; a run this late is recorded as missed instead.
const LATE = sql`interval '1 hour'`;
const SCAN_INTERVAL_MS = 30_000;
const RENEW_INTERVAL_MS = 20_000;
const FIRE_BATCH = 50;
const MAX_ATTEMPTS = 3;
const MAX_RUNNING = 4;
const MAX_STEPS = 16;
const RUN_TIMEOUT_MS = 10 * 60_000;
const TOOL_TIMEOUT_MS = 2 * 60_000;

// A run works without the user: it reads and reports, and changes nothing.
const runTools = {
  web_search: webTools.web_search,
  memory_read: memoryTools.memory_read,
  conversation_search: historyTools.conversation_search,
  ...timeTools,
  ...searchTools,
};

const Report = z.object({
  message: z
    .string()
    .nullable()
    .describe("Your report to the user, or null when the schedule says to stay quiet"),
});

type Run = typeof jobRuns.$inferSelect & { owner: string };

class Stopped extends Error {}

const log = logger.child({ module: "jobs" });
const shutdown = new AbortController();
const tasks = new Set<Promise<void>>();
let scanTimer: Timer | undefined;

export function startJobs(): void {
  const schedule = () => {
    if (shutdown.signal.aborted) return;
    scanTimer = setTimeout(
      () => {
        tick()
          .catch((error: unknown) => log.error({ err: error }, "job scan failed"))
          .finally(schedule);
      },
      SCAN_INTERVAL_MS * (0.5 + Math.random()),
    );
  };
  schedule();
}

export async function stopJobs(): Promise<void> {
  shutdown.abort(new Stopped());
  clearTimeout(scanTimer);
  await Promise.allSettled(tasks);
}

/** Fires due schedules and starts runs; resolves once started, not once finished. */
export async function tick(): Promise<void> {
  await fireDue();
  await giveUp();
  while (tasks.size < MAX_RUNNING && !shutdown.signal.aborted) {
    const run = await claim();
    if (!run) break;
    const task = execute(run).finally(() => tasks.delete(task));
    tasks.add(task);
  }
}

/** Resolves when every run this process started has finished. */
export async function settle(): Promise<void> {
  while (tasks.size > 0) await Promise.allSettled(tasks);
}

async function fireDue(): Promise<void> {
  await db.transaction(async (tx) => {
    const due = await tx
      .select({
        job: jobs,
        timeZone: users.timeZone,
        isLate: sql<boolean>`clock_timestamp() - ${jobs.nextRunAt} > ${LATE}`,
      })
      .from(jobs)
      .innerJoin(agents, eq(agents.id, jobs.agentId))
      .innerJoin(users, eq(users.id, agents.userId))
      .where(and(eq(jobs.enabled, true), lte(jobs.nextRunAt, sql`clock_timestamp()`)))
      .limit(FIRE_BATCH)
      .for("update", { of: jobs, skipLocked: true });

    for (const { job, timeZone, isLate } of due) {
      if (!job.nextRunAt) continue;
      await tx
        .insert(jobRuns)
        .values({
          jobId: job.id,
          scheduledFor: job.nextRunAt,
          ...(isLate ? { error: "missed" as const, finishedAt: sql`clock_timestamp()` } : {}),
        })
        .onConflictDoNothing();
      await tx
        .update(jobs)
        .set({ nextRunAt: nextAfterNow(job, timeZone ?? "UTC") })
        .where(eq(jobs.id, job.id));
    }
  });
}

async function giveUp(): Promise<void> {
  await db
    .update(jobRuns)
    .set({ finishedAt: sql`clock_timestamp()`, error: "failed", owner: null, leaseUntil: null })
    .where(
      and(
        isNull(jobRuns.finishedAt),
        gte(jobRuns.attempts, MAX_ATTEMPTS),
        or(isNull(jobRuns.owner), lt(jobRuns.leaseUntil, sql`clock_timestamp()`)),
      ),
    );
}

async function claim(): Promise<Run | undefined> {
  const owner = crypto.randomUUID();
  const next = db
    .select({ id: jobRuns.id })
    .from(jobRuns)
    .where(
      and(
        isNull(jobRuns.finishedAt),
        lt(jobRuns.attempts, MAX_ATTEMPTS),
        or(isNull(jobRuns.owner), lt(jobRuns.leaseUntil, sql`clock_timestamp()`)),
      ),
    )
    .orderBy(jobRuns.id)
    .limit(1)
    .for("update", { skipLocked: true });
  const [run] = await db
    .update(jobRuns)
    .set({
      owner,
      leaseUntil: sql`clock_timestamp() + ${LEASE}`,
      attempts: sql`${jobRuns.attempts} + 1`,
    })
    .where(inArray(jobRuns.id, next))
    .returning();
  return run ? { ...run, owner } : undefined;
}

async function execute(run: Run): Promise<void> {
  const mine = and(eq(jobRuns.id, run.id), eq(jobRuns.owner, run.owner));
  const lost = new AbortController();
  const signal = AbortSignal.any([
    shutdown.signal,
    lost.signal,
    AbortSignal.timeout(RUN_TIMEOUT_MS),
  ]);
  const renew = async (): Promise<void> => {
    try {
      const rows = await db
        .update(jobRuns)
        .set({ leaseUntil: sql`clock_timestamp() + ${LEASE}` })
        .where(mine)
        .returning({ id: jobRuns.id });
      if (rows.length === 0) lost.abort();
    } catch {
      // A failed renewal shows up as an expired lease that another instance reclaims.
    }
  };
  const heartbeat = setInterval(() => void renew(), RENEW_INTERVAL_MS);

  let catalog: Catalog | undefined;
  try {
    const [setup] = await db
      .select({
        job: jobs,
        agent: { id: agents.id, name: agents.name, instructions: agents.instructions },
        user: { id: users.id, name: users.name, timeZone: users.timeZone },
      })
      .from(jobs)
      .innerJoin(agents, eq(agents.id, jobs.agentId))
      .innerJoin(users, eq(users.id, agents.userId))
      .where(eq(jobs.id, run.jobId));
    if (!setup) return;
    const { job, agent, user } = setup;
    const timeZone = user.timeZone ?? "UTC";

    catalog = openCatalog(user.id, { readOnly: true });
    const result = await generateText({
      model,
      instructions: scheduledRunInstructionsFor(agent, user.name),
      messages: [{ role: "user", content: await brief(run, job, user.id, timeZone) }],
      tools: runTools,
      toolsContext: {
        time: { timeZone },
        memory_read: { userId: user.id, timeZone },
        // A run sees none of the conversation, so all of it is out of view.
        conversation_search: { agentId: agent.id, timeZone, cutId: Number.MAX_SAFE_INTEGER },
        tool_search: catalog,
        tool_call: catalog,
      },
      output: Output.object({ schema: Report }),
      stopWhen: stepCountIs(MAX_STEPS),
      providerOptions,
      abortSignal: signal,
      timeout: { toolMs: TOOL_TIMEOUT_MS },
    });
    const report = result.output.message?.trim() ?? "";
    const message = report.length > 0 ? report : null;

    const finished = await db.transaction(async (tx) => {
      const rows = await tx
        .update(jobRuns)
        .set({
          finishedAt: sql`clock_timestamp()`,
          outcome: message,
          model: result.response.modelId,
          usage: result.totalUsage,
          owner: null,
          leaseUntil: null,
        })
        .where(mine)
        .returning({ id: jobRuns.id });
      if (rows.length === 0) return false;
      if (!message && !job.rrule) await tx.delete(jobs).where(eq(jobs.id, job.id));
      return true;
    });
    log.info(
      {
        jobId: job.id,
        runId: run.id,
        tools: result.steps.flatMap((step) => step.toolCalls.map((call) => call.toolName)),
        reported: Boolean(message),
      },
      "job ran",
    );
    if (finished && message) wake(agent.id);
  } catch (error) {
    if (shutdown.signal.aborted) {
      // Shutting down: hand the run to another instance right away.
      await db
        .update(jobRuns)
        .set({ leaseUntil: sql`clock_timestamp()` })
        .where(mine);
    } else {
      log.error({ err: error, jobId: run.jobId, runId: run.id }, "job run failed");
      await db.update(jobRuns).set({ owner: null, leaseUntil: null }).where(mine);
    }
  } finally {
    clearInterval(heartbeat);
    await catalog?.close();
  }
}

/** What the run is told: the time, the schedule, and what it reported last time. */
async function brief(
  run: Run,
  job: typeof jobs.$inferSelect,
  userId: string,
  timeZone: string,
): Promise<{ type: "text"; text: string }[]> {
  const now = Temporal.Now.zonedDateTimeISO(timeZone);
  const [previous] = await db
    .select({ outcome: jobRuns.outcome })
    .from(jobRuns)
    .where(and(eq(jobRuns.jobId, job.id), ne(jobRuns.id, run.id), isNotNull(jobRuns.outcome)))
    .orderBy(desc(jobRuns.scheduledFor))
    .limit(1);
  const remembered = await memory.noteFor({ userId, timeZone });
  return [
    { type: "text", text: `Today's date is ${dateIn(timeZone, new Date())}.` },
    ...(remembered ? [{ type: "text" as const, text: remembered }] : []),
    { type: "text", text: `It is now ${now.toString({ smallestUnit: "minute" })}.` },
    { type: "text", text: `Schedule: ${job.instruction}` },
    ...(previous?.outcome
      ? [{ type: "text" as const, text: `Your report last time:\n${previous.outcome}` }]
      : []),
  ];
}

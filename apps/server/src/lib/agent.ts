import {
  APICallError,
  generateText,
  type LanguageModelUsage,
  type ModelMessage,
  NoSuchToolError,
  RetryError,
  type TextPart,
  type ToolCallRepairFunction,
  type UserContent,
} from "ai";
import { and, asc, desc, eq, isNotNull, isNull, lt, or, sql } from "drizzle-orm";
import { db, type Transaction } from "../db";
import { agents, messages, queuedMessages } from "../db/schema/agents";
import { users } from "../db/schema/auth";
import { jobRuns, jobs } from "../db/schema/jobs";
import { instructionsFor } from "./ai/instructions";
import { model, providerOptions } from "./ai/providers";
import { tools } from "./ai/tools";
import { computerContext } from "./ai/tools/computer";
import { hasReadOutside } from "./ai/trust";
import { compactScreens, type Computers, openComputers } from "./computers";
import * as compaction from "./compaction";
import { bubblesOf } from "./conversation";
import { dateIn } from "./dates";
import { notify } from "./events";
import * as files from "./files";
import { type Catalog, openCatalog } from "./integrations/catalog";
import { listConnections, NOTE_PREFIX, noteFor } from "./integrations/connections";
import { scheduleNoteChange } from "./jobs/schedules";
import { logger } from "./logger";
import * as memory from "./memory";
import * as push from "./push";
import * as search from "./search";

const LEASE = sql`interval '60 seconds'`;
const LEASE_DEADLINE_MS = 45_000;
const RENEW_INTERVAL_MS = 20_000;
const SCAN_INTERVAL_MS = 30_000;
const SCAN_BATCH = 20;
const STEP_TIMEOUT_MS = 10 * 60_000;
const TOOL_TIMEOUT_MS = 2 * 60_000;
const MAX_STEPS = 100;

export type Agent = typeof agents.$inferSelect;

const log = logger.child({ module: "agent" });

class LeaseLost extends Error {}
class ShuttingDown extends Error {}

const shutdown = new AbortController();
const running = new Set<string>();
const tasks = new Set<Promise<void>>();
let scanTimer: Timer | undefined;

export async function createAgent(
  userId: string,
  input: { name: string; instructions: string },
): Promise<Agent> {
  return db.transaction(async (tx) => {
    const [agent] = await tx
      .insert(agents)
      .values({ userId, ...input })
      .returning();
    if (!agent) throw new Error("agent insert returned no row");
    await notify(tx, agent.id);
    return agent;
  });
}

type AgentChanges = Partial<Pick<Agent, "name" | "instructions" | "avatarId">>;

/** Returns the agent before and after the change, or `undefined` when the user has no such agent. */
export async function updateAgent(
  userId: string,
  agentId: string,
  changes: AgentChanges,
): Promise<{ before: Agent; after: Agent } | undefined> {
  return db.transaction(async (tx) => {
    const [before] = await tx
      .select()
      .from(agents)
      .where(and(eq(agents.id, agentId), eq(agents.userId, userId)))
      .for("update");
    if (!before) return undefined;
    const [after] = await tx.update(agents).set(changes).where(eq(agents.id, agentId)).returning();
    if (!after) throw new Error("agent update returned no row");
    await notify(tx, agentId);
    return { before, after };
  });
}

/**
 * Deletes the agent with its conversation. A run in progress loses its lease and stops; the
 * agent's files are unlinked for the sweeper.
 */
export async function deleteAgent(userId: string, agentId: string): Promise<Agent | undefined> {
  return db.transaction(async (tx) => {
    const [deleted] = await tx
      .delete(agents)
      .where(and(eq(agents.id, agentId), eq(agents.userId, userId)))
      .returning();
    if (deleted) await notify(tx, agentId);
    return deleted;
  });
}

/** Throws `files.FileNotFound` unless every file id is an upload to this agent. */
export async function enqueue(
  agentId: string,
  input: { text: string; fileIds: readonly string[] },
  timeZone = "UTC",
): Promise<void> {
  await queue(agentId, { ...input, timeZone });
}

export async function resumeAfterConnect(agentId: string, timeZone = "UTC"): Promise<void> {
  await queue(agentId, { timeZone });
}

async function queue(
  agentId: string,
  input: { text?: string; fileIds?: readonly string[]; timeZone: string },
): Promise<void> {
  const [agent] = await db
    .select({
      userId: agents.userId,
      connectionsNote: agents.connectionsNote,
      schedulesNote: agents.schedulesNote,
      memoryNote: agents.memoryNote,
    })
    .from(agents)
    .where(eq(agents.id, agentId));
  if (!agent) return;

  const parts: Exclude<UserContent, string> = [];
  const date = await datePart(db, agentId, input.timeZone);
  if (date) parts.push(date);

  const connections = await listConnections(agent.userId);
  const note =
    connections.length > 0 || agent.connectionsNote !== null ? noteFor(connections) : undefined;
  const hidden = input.text === undefined;
  if (hidden && !note) return;
  if (note && (hidden || note !== agent.connectionsNote)) parts.push({ type: "text", text: note });

  await db.transaction(async (tx) => {
    const schedules = await scheduleNoteChange(tx, agentId, agent.schedulesNote);
    if (schedules) parts.push({ type: "text", text: schedules });
    const remembered = await memory.noteChange(
      tx,
      agentId,
      { userId: agent.userId, timeZone: input.timeZone },
      agent.memoryNote,
    );
    if (remembered) parts.push({ type: "text", text: remembered });
    const attached = await files.attach(tx, agentId, input.fileIds ?? []);
    parts.push(...attached.map(files.reference));
    if (input.text) parts.push({ type: "text", text: input.text });
    const content: UserContent = parts.length === 1 && input.text ? input.text : parts;
    await tx.insert(queuedMessages).values({ agentId, hidden, message: { role: "user", content } });
    if (note !== undefined && note !== agent.connectionsNote) {
      await tx.update(agents).set({ connectionsNote: note }).where(eq(agents.id, agentId));
    }
  });
  launch(agentId, { onlyIfQueued: true });
}

async function datePart(
  executor: Transaction | typeof db,
  agentId: string,
  timeZone: string,
): Promise<TextPart | undefined> {
  const today = dateIn(timeZone, new Date());
  const last = await lastUserMessageAt(executor, agentId);
  if (last && dateIn(timeZone, last) === today) return undefined;
  return { type: "text", text: `Today's date is ${today}.` };
}

async function lastUserMessageAt(
  executor: Transaction | typeof db,
  agentId: string,
): Promise<Date | undefined> {
  const [queued] = await executor
    .select({ createdAt: queuedMessages.createdAt })
    .from(queuedMessages)
    .where(eq(queuedMessages.agentId, agentId))
    .orderBy(desc(queuedMessages.id))
    .limit(1);
  if (queued) return queued.createdAt;

  const [sent] = await executor
    .select({ createdAt: messages.createdAt })
    .from(messages)
    .where(and(eq(messages.agentId, agentId), sql`${messages.message}->>'role' = 'user'`))
    .orderBy(desc(messages.id))
    .limit(1);
  return sent?.createdAt;
}

/** Starts a run if the agent is free, such as to deliver a finished job's report. */
export function wake(agentId: string): void {
  launch(agentId, { onlyIfQueued: false });
}

export function startAgents(): void {
  const schedule = () => {
    if (shutdown.signal.aborted) return;
    const jittered = SCAN_INTERVAL_MS * (0.5 + Math.random());
    scanTimer = setTimeout(() => {
      scan()
        .catch((error: unknown) => log.error({ err: error }, "scan failed"))
        .finally(schedule);
    }, jittered);
  };
  schedule();
}

export async function stopAgents(): Promise<void> {
  shutdown.abort(new ShuttingDown());
  clearTimeout(scanTimer);
  await Promise.allSettled(tasks);
}

export async function scan(): Promise<void> {
  const [expired, waiting, reporting] = await Promise.all([
    db
      .select({ id: agents.id })
      .from(agents)
      .where(lt(agents.runLeaseUntil, sql`clock_timestamp()`))
      .limit(SCAN_BATCH),
    db
      .selectDistinct({ id: queuedMessages.agentId })
      .from(queuedMessages)
      .innerJoin(agents, eq(agents.id, queuedMessages.agentId))
      .where(isNull(agents.runOwner))
      .limit(SCAN_BATCH),
    db
      .selectDistinct({ id: jobs.agentId })
      .from(jobRuns)
      .innerJoin(jobs, eq(jobs.id, jobRuns.jobId))
      .innerJoin(agents, eq(agents.id, jobs.agentId))
      .where(and(isNotNull(jobRuns.outcome), isNull(jobRuns.messageId), isNull(agents.runOwner)))
      .limit(SCAN_BATCH),
  ]);
  for (const { id } of expired) launch(id, { onlyIfQueued: false });
  for (const { id } of waiting) launch(id, { onlyIfQueued: true });
  for (const { id } of reporting) launch(id, { onlyIfQueued: false });
}

function launch(agentId: string, options: { onlyIfQueued: boolean }): void {
  const task = start(agentId, options).finally(() => tasks.delete(task));
  tasks.add(task);
}

async function start(agentId: string, options: { onlyIfQueued: boolean }): Promise<void> {
  if (shutdown.signal.aborted || running.has(agentId)) return;
  running.add(agentId);
  try {
    const owner = await claim(agentId, options.onlyIfQueued);
    if (owner) await execute(agentId, owner);
  } catch (error) {
    log.error({ err: error, agentId }, "run failed to start or finish");
  } finally {
    running.delete(agentId);
  }
}

async function claim(agentId: string, onlyIfQueued: boolean): Promise<string | undefined> {
  const owner = crypto.randomUUID();
  const free = or(isNull(agents.runOwner), lt(agents.runLeaseUntil, sql`clock_timestamp()`));
  const queued = sql`exists (select 1 from ${queuedMessages} where ${queuedMessages.agentId} = ${agents.id})`;

  const claimed = await db.transaction(async (tx) => {
    const rows = await tx
      .update(agents)
      .set({ runOwner: owner, runLeaseUntil: sql`clock_timestamp() + ${LEASE}` })
      .where(and(eq(agents.id, agentId), free, onlyIfQueued ? queued : undefined))
      .returning({ id: agents.id });
    if (rows.length === 0) return false;
    await notify(tx, agentId);
    return true;
  });
  return claimed ? owner : undefined;
}

async function execute(agentId: string, owner: string): Promise<void> {
  const lost = new AbortController();
  // A run claimed during shutdown starts aborted and hands its lease straight back to the scanner.
  const signal = AbortSignal.any([shutdown.signal, lost.signal]);
  const lease = holdLease(agentId, owner, lost);
  let catalog: Catalog | undefined;
  try {
    signal.throwIfAborted();
    const { name, instructions, timeZone, userId, userName } = await loadSetup(agentId);
    if (await isAtRest(agentId)) {
      const reports = await lease.write((tx) => deliverReports(tx, agentId, timeZone));
      for (const report of reports) {
        void push.send(userId, {
          title: name,
          body: report.preview,
          agentId,
          collapseId: report.id,
        });
      }
    }
    catalog = openCatalog(userId);
    const fileCache = new Map<string, Uint8Array>();
    let computers: Computers | undefined;
    for (let step = 0; step < MAX_STEPS; step++) {
      signal.throwIfAborted();
      if (await lease.write((tx) => drainQueue(tx, agentId))) await catalog.reload();

      let conversation = await compaction.load(agentId);
      const redacted = await compaction.applyRedactions({
        agentId,
        userId,
        userName,
        timeZone,
        conversation,
        write: lease.write,
        signal,
      });
      if (redacted) conversation = await compaction.load(agentId);
      const compacted = await compaction.maybeCompact({
        agentId,
        userId,
        userName,
        timeZone,
        conversation,
        write: lease.write,
        signal,
      });
      if (compacted) conversation = await compaction.load(agentId);
      const history = conversation.messages;
      const last = history.at(-1);
      if (!last || last.role === "assistant") {
        if (await lease.finish()) return;
        continue;
      }

      computers ??= openComputers(userId, compaction.computerFloor(conversation));
      const isTainted = await hasReadOutside(agentId);
      const memoryWriter = { userId, agentId, timeZone, isTainted };
      const startedAt = performance.now();
      const result = await generateText({
        model,
        instructions,
        messages: await files.forModel(history, fileCache),
        tools,
        toolsContext: {
          time: { timeZone },
          schedule_create: { agentId, timeZone, isTainted },
          schedule_update: { agentId, timeZone, isTainted },
          schedule_cancel: { agentId, timeZone },
          read_file: { agentId },
          memory_read: { userId, timeZone },
          conversation_search: {
            agentId,
            timeZone,
            cutId: conversation.summary?.uptoMessageId ?? 0,
          },
          memory_save: memoryWriter,
          memory_forget: memoryWriter,
          memory_rename: memoryWriter,
          tool_search: catalog,
          tool_call: catalog,
          ...computerContext(computers),
        },
        repairToolCall: routeThroughToolCall,
        providerOptions,
        abortSignal: signal,
        timeout: { stepMs: STEP_TIMEOUT_MS, toolMs: TOOL_TIMEOUT_MS },
      });
      log.info(
        {
          agentId,
          model: result.response.modelId,
          finishReason: result.finishReason,
          tools: result.toolCalls.map((call) => call.toolName),
          inputTokens: result.usage.inputTokens,
          outputTokens: result.usage.outputTokens,
          ms: Math.round(performance.now() - startedAt),
        },
        "step",
      );
      await lease.write((tx) =>
        saveResponse(tx, agentId, {
          messages: result.response.messages,
          model: result.response.modelId,
          usage: result.usage,
        }),
      );
    }
    await lease.release();
  } catch (error) {
    if (signal.reason instanceof ShuttingDown) {
      await lease.stop();
      await lease.expire();
    } else if (error instanceof LeaseLost || signal.reason instanceof LeaseLost) {
      log.warn({ agentId }, "lease lost");
    } else if (!signal.aborted) {
      log.error({ err: error, agentId }, "run failed");
      if (isRetryable(error)) {
        await lease.stop();
        await lease.expire();
      } else {
        await lease.release();
      }
    }
  } finally {
    await catalog?.close();
    await lease.stop();
  }
}

// Claude learned to call a tool it found by name; such a call runs through tool_call instead.
const routeThroughToolCall: ToolCallRepairFunction<typeof tools> = ({ toolCall, error }) => {
  if (!NoSuchToolError.isInstance(error)) return Promise.resolve(null);
  let input: unknown;
  try {
    input = JSON.parse(toolCall.input || "{}");
  } catch {
    return Promise.resolve(null);
  }
  return Promise.resolve({
    ...toolCall,
    toolName: "tool_call",
    input: JSON.stringify({ name: toolCall.toolName, input }),
  });
};

function holdLease(agentId: string, owner: string, lost: AbortController) {
  const mine = and(eq(agents.id, agentId), eq(agents.runOwner, owner));
  const giveUp = () => lost.abort(new LeaseLost());

  const renew = async (tx: Transaction | typeof db): Promise<boolean> => {
    const rows = await tx
      .update(agents)
      .set({ runLeaseUntil: sql`clock_timestamp() + ${LEASE}` })
      .where(mine)
      .returning({ id: agents.id });
    return rows.length > 0;
  };

  // Renewal only proves the process is alive; the watchdog stops a run that cannot renew in time.
  const watchdog = setTimeout(giveUp, LEASE_DEADLINE_MS);
  const beat = async (): Promise<void> => {
    try {
      if (await renew(db)) watchdog.refresh();
      else giveUp();
    } catch {
      // A failed renewal is left to the watchdog.
    }
  };
  let renewing: Promise<void> | undefined;
  const heartbeat = setInterval(() => {
    renewing ??= beat().finally(() => {
      renewing = undefined;
    });
  }, RENEW_INTERVAL_MS);

  const write = async <T>(fn: (tx: Transaction) => Promise<T>): Promise<T> => {
    const result = await db.transaction(async (tx) => {
      if (!(await renew(tx))) throw new LeaseLost();
      return fn(tx);
    });
    watchdog.refresh();
    return result;
  };

  const finish = (): Promise<boolean> =>
    db.transaction(async (tx) => {
      const [held] = await tx.select({ id: agents.id }).from(agents).where(mine).for("update");
      if (!held) throw new LeaseLost();

      // Checked only after the row lock: a sender that commits later blocks on the lock and claims next.
      const [pending] = await tx
        .select({ id: queuedMessages.id })
        .from(queuedMessages)
        .where(eq(queuedMessages.agentId, agentId))
        .limit(1);
      if (pending) return false;

      await compactScreens(tx, agentId);
      await tx.update(agents).set({ runOwner: null, runLeaseUntil: null }).where(mine);
      await notify(tx, agentId);
      return true;
    });

  const release = async (): Promise<void> => {
    await db.transaction(async (tx) => {
      await tx.update(agents).set({ runOwner: null, runLeaseUntil: null }).where(mine);
      await notify(tx, agentId);
    });
  };

  // Left to the scanner instead of released: the conversation may still need a reply.
  const expire = async (): Promise<void> => {
    await db
      .update(agents)
      .set({ runLeaseUntil: sql`clock_timestamp()` })
      .where(mine);
  };

  // Waits for an in-flight renewal so it cannot extend the lease after expire().
  const stop = async (): Promise<void> => {
    clearInterval(heartbeat);
    clearTimeout(watchdog);
    await renewing;
  };

  return { write, finish, release, expire, stop };
}

/** True when the moved messages say the user's connections changed. */
async function drainQueue(tx: Transaction, agentId: string): Promise<boolean> {
  const queued = await tx
    .delete(queuedMessages)
    .where(eq(queuedMessages.agentId, agentId))
    .returning();
  if (queued.length === 0) return false;

  queued.sort((a, b) => a.id - b.id);
  const inserted = await tx
    .insert(messages)
    .values(
      queued.map(({ message, hidden, createdAt }) => ({ agentId, message, hidden, createdAt })),
    )
    .returning({
      id: messages.id,
      message: messages.message,
      hidden: messages.hidden,
      createdAt: messages.createdAt,
    });
  await search.recordUserMessages(
    tx,
    agentId,
    inserted.toSorted((a, b) => a.id - b.id),
  );
  await notify(tx, agentId);
  return queued.some(({ message }) => carriesNote(message));
}

function carriesNote(message: ModelMessage): boolean {
  return (
    typeof message.content !== "string" &&
    message.content.some((part) => part.type === "text" && part.text.startsWith(NOTE_PREFIX))
  );
}

async function saveResponse(
  tx: Transaction,
  agentId: string,
  response: { messages: ModelMessage[]; model: string; usage: LanguageModelUsage },
): Promise<void> {
  const { model: responseModel, usage } = response;
  const inserted = await tx
    .insert(messages)
    .values(
      response.messages.map((message) =>
        message.role === "assistant"
          ? { agentId, message, model: responseModel, usage }
          : { agentId, message },
      ),
    )
    .returning({ id: messages.id });
  await search.recordReply(
    tx,
    agentId,
    inserted.map(({ id }) => id),
    response.messages,
  );
  await notify(tx, agentId);
}

/** True when the conversation ends with the agent's reply, so a report can follow it. */
async function isAtRest(agentId: string): Promise<boolean> {
  const [last] = await db
    .select({ role: sql<string>`${messages.message}->>'role'` })
    .from(messages)
    .where(eq(messages.agentId, agentId))
    .orderBy(desc(messages.id))
    .limit(1);
  return !last || last.role === "assistant";
}

/**
 * Writes finished jobs' reports as the agent's own turns: a hidden user marker saying which
 * schedule ran, then the report. A one-time schedule is done once its report is delivered.
 */
async function deliverReports(
  tx: Transaction,
  agentId: string,
  timeZone: string,
): Promise<{ id: string; preview: string }[]> {
  const pending = await tx
    .select({ runId: jobRuns.id, outcome: jobRuns.outcome, job: jobs })
    .from(jobRuns)
    .innerJoin(jobs, eq(jobs.id, jobRuns.jobId))
    .where(and(eq(jobs.agentId, agentId), isNotNull(jobRuns.outcome), isNull(jobRuns.messageId)))
    .orderBy(asc(jobRuns.id))
    .for("update", { of: jobRuns });

  const delivered: { id: string; preview: string }[] = [];
  for (const { runId, outcome, job } of pending) {
    if (!outcome) continue;
    const date = await datePart(tx, agentId, timeZone);
    const marker: ModelMessage = {
      role: "user",
      content: [...(date ? [date] : []), { type: "text", text: `Scheduled run: ${job.title}` }],
    };
    const report: ModelMessage = { role: "assistant", content: [{ type: "text", text: outcome }] };
    const [markerRow, reply] = await tx
      .insert(messages)
      .values([
        { agentId, message: marker, hidden: true },
        { agentId, message: report },
      ])
      .returning({ id: messages.id, createdAt: messages.createdAt });
    if (!markerRow || !reply) throw new Error("report insert returned no row");
    await search.recordReport(tx, agentId, {
      markerId: markerRow.id,
      replyId: reply.id,
      title: job.title,
      text: outcome,
      at: reply.createdAt,
    });
    if (job.rrule) {
      await tx.update(jobRuns).set({ messageId: reply.id }).where(eq(jobRuns.id, runId));
    } else {
      await tx.delete(jobs).where(eq(jobs.id, job.id));
    }
    delivered.push({ id: String(runId), preview: bubblesOf(report)[0] ?? outcome });
  }
  if (delivered.length > 0) await notify(tx, agentId);
  return delivered;
}

async function loadSetup(agentId: string): Promise<{
  name: string;
  instructions: string;
  timeZone: string;
  userId: string;
  userName: string;
}> {
  const [row] = await db
    .select({
      userId: agents.userId,
      name: agents.name,
      instructions: agents.instructions,
      userName: users.name,
      timeZone: users.timeZone,
    })
    .from(agents)
    .innerJoin(users, eq(users.id, agents.userId))
    .where(eq(agents.id, agentId));
  if (!row) throw new LeaseLost();

  return {
    name: row.name,
    instructions: instructionsFor(row, row.userName),
    timeZone: row.timeZone ?? "UTC",
    userId: row.userId,
    userName: row.userName,
  };
}

// Retried by the scanner; anything else is dropped so a request that can never succeed does not loop.
function isRetryable(error: unknown): boolean {
  if (RetryError.isInstance(error)) return error.reason === "maxRetriesExceeded";
  if (APICallError.isInstance(error)) return error.isRetryable;
  return error instanceof Error && error.name === "TimeoutError";
}

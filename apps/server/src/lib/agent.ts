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
import { and, asc, desc, eq, isNull, lt, or, sql } from "drizzle-orm";
import { db, type Transaction } from "../db";
import { agents, messages, queuedMessages } from "../db/schema/agents";
import { users } from "../db/schema/auth";
import { model, providerOptions } from "./ai/providers";
import { tools } from "./ai/tools";
import { dateIn } from "./dates";
import { notify } from "./events";
import { type Catalog, openCatalog } from "./integrations/catalog";
import { listConnections, NOTE_PREFIX, noteFor } from "./integrations/connections";
import { logger } from "./logger";

const LEASE = sql`interval '60 seconds'`;
const LEASE_DEADLINE_MS = 45_000;
const RENEW_INTERVAL_MS = 20_000;
const SCAN_INTERVAL_MS = 30_000;
const SCAN_BATCH = 20;
const STEP_TIMEOUT_MS = 10 * 60_000;
const TOOL_TIMEOUT_MS = 2 * 60_000;
const MAX_STEPS = 32;

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
  const [agent] = await db
    .insert(agents)
    .values({ userId, ...input })
    .returning();
  if (!agent) throw new Error("agent insert returned no row");
  return agent;
}

export async function enqueue(agentId: string, text: string, timeZone = "UTC"): Promise<void> {
  await queue(agentId, { text, timeZone });
}

export async function resumeAfterConnect(agentId: string, timeZone = "UTC"): Promise<void> {
  await queue(agentId, { timeZone });
}

async function queue(agentId: string, input: { text?: string; timeZone: string }): Promise<void> {
  const [agent] = await db
    .select({ userId: agents.userId, connectionsNote: agents.connectionsNote })
    .from(agents)
    .where(eq(agents.id, agentId));
  if (!agent) return;

  const parts: TextPart[] = [];
  const today = dateIn(input.timeZone, new Date());
  const last = await lastUserMessageAt(agentId);
  if (!last || dateIn(input.timeZone, last) !== today) {
    parts.push({ type: "text", text: `Today's date is ${today}.` });
  }

  const connections = await listConnections(agent.userId);
  const note =
    connections.length > 0 || agent.connectionsNote !== null ? noteFor(connections) : undefined;
  const hidden = input.text === undefined;
  if (hidden && !note) return;
  if (note && (hidden || note !== agent.connectionsNote)) parts.push({ type: "text", text: note });
  if (input.text !== undefined) parts.push({ type: "text", text: input.text });

  const content: UserContent = parts.length === 1 && input.text !== undefined ? input.text : parts;
  await db.transaction(async (tx) => {
    await tx.insert(queuedMessages).values({ agentId, hidden, message: { role: "user", content } });
    if (note !== undefined && note !== agent.connectionsNote) {
      await tx.update(agents).set({ connectionsNote: note }).where(eq(agents.id, agentId));
    }
  });
  launch(agentId, { onlyIfQueued: true });
}

async function lastUserMessageAt(agentId: string): Promise<Date | undefined> {
  const [queued] = await db
    .select({ createdAt: queuedMessages.createdAt })
    .from(queuedMessages)
    .where(eq(queuedMessages.agentId, agentId))
    .orderBy(desc(queuedMessages.id))
    .limit(1);
  if (queued) return queued.createdAt;

  const [sent] = await db
    .select({ createdAt: messages.createdAt })
    .from(messages)
    .where(and(eq(messages.agentId, agentId), sql`${messages.message}->>'role' = 'user'`))
    .orderBy(desc(messages.id))
    .limit(1);
  return sent?.createdAt;
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
  const [expired, waiting] = await Promise.all([
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
  ]);
  for (const { id } of expired) launch(id, { onlyIfQueued: false });
  for (const { id } of waiting) launch(id, { onlyIfQueued: true });
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
    const { instructions, timeZone, userId } = await loadSetup(agentId);
    catalog = openCatalog(userId);
    for (let step = 0; step < MAX_STEPS; step++) {
      signal.throwIfAborted();
      if (await lease.write((tx) => drainQueue(tx, agentId))) await catalog.reload();

      const history = await loadHistory(agentId);
      const last = history.at(-1);
      if (!last || last.role === "assistant") {
        if (await lease.finish()) return;
        continue;
      }

      const startedAt = performance.now();
      const result = await generateText({
        model,
        instructions,
        messages: history,
        tools,
        toolsContext: { time: { timeZone }, tool_search: catalog, tool_call: catalog },
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
  await tx
    .insert(messages)
    .values(
      queued.map(({ message, hidden, createdAt }) => ({ agentId, message, hidden, createdAt })),
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
  await tx
    .insert(messages)
    .values(
      response.messages.map((message) =>
        message.role === "assistant"
          ? { agentId, message, model: responseModel, usage }
          : { agentId, message },
      ),
    );
  await notify(tx, agentId);
}

async function loadHistory(agentId: string): Promise<ModelMessage[]> {
  const rows = await db
    .select({ message: messages.message })
    .from(messages)
    .where(eq(messages.agentId, agentId))
    .orderBy(asc(messages.id));

  return rows.map((row) => row.message);
}

async function loadSetup(
  agentId: string,
): Promise<{ instructions: string; timeZone: string; userId: string }> {
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

  const parts = [
    `You are ${row.name}, a personal agent for ${row.userName}.`,
    "When an answer relies on the web, cite the pages you used.",
  ];
  if (row.instructions) parts.push(`${row.userName}'s instructions:\n${row.instructions}`);
  return {
    instructions: parts.join("\n\n"),
    timeZone: row.timeZone ?? "UTC",
    userId: row.userId,
  };
}

// Retried by the scanner; anything else is dropped so a request that can never succeed does not loop.
function isRetryable(error: unknown): boolean {
  if (RetryError.isInstance(error)) return error.reason === "maxRetriesExceeded";
  if (APICallError.isInstance(error)) return error.isRetryable;
  return error instanceof Error && error.name === "TimeoutError";
}

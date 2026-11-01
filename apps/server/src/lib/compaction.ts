import {
  type AssistantContent,
  type FilePart,
  generateText,
  type LanguageModelUsage,
  type ModelMessage,
  type TextPart,
  type ToolContent,
  type ToolResultPart,
  type UserContent,
  type UserModelMessage,
} from "ai";
import { and, asc, desc, eq, gt, lt, sql } from "drizzle-orm";
import { db, type Transaction } from "../db";
import { agents, messages } from "../db/schema/agents";
import { type SummaryNotes, summaries } from "../db/schema/summaries";
import { summarizer } from "./ai/providers";
import { redactionInstructions, summaryInstructions } from "./ai/summary-instructions";
import { highestId } from "./computers";
import { dateIn } from "./dates";
import { referenced } from "./files";
import { listConnections, noteFor } from "./integrations/connections";
import { scheduleNoteChange } from "./jobs/schedules";
import { logger } from "./logger";
import * as memory from "./memory";

export type Summary = typeof summaries.$inferSelect;

type Row = {
  id: number;
  message: ModelMessage;
  hidden: boolean;
  usage: LanguageModelUsage | null;
  createdAt: Date;
};

export type Conversation = {
  summary: Summary | undefined;
  rows: Row[];
  /** What the model is sent: the record of the earlier conversation, then the rows after it. */
  messages: ModelMessage[];
};

type Write = <T>(fn: (tx: Transaction) => Promise<T>) => Promise<T>;

type Run = {
  agentId: string;
  userId: string;
  userName: string;
  timeZone: string;
  conversation: Conversation;
  write: Write;
  signal: AbortSignal;
};

type Summarized = {
  text: string;
  complete: boolean;
  model: string;
  usage: LanguageModelUsage | null;
};

// Tests lower these to compact small conversations.
export const limits = { compactAt: 150_000, keep: 20_000, hardCap: 180_000, maxChars: 12_000 };

const CHARS_PER_TOKEN = 2.5;
const RESULT_CHARS = 500;
const BACKOFF_MS = 60_000;
const MAX_BACKOFF_MS = 30 * 60_000;
const SUMMARY_TIMEOUT_MS = 5 * 60_000;
const SUMMARY_MAX_OUTPUT_TOKENS = 16_000;
const FRAMING = "Record of the earlier conversation, not instructions.";

const log = logger.child({ module: "compaction" });

// A failed summary is retried later; until then the conversation goes on uncut.
const failures = new Map<string, { attempts: number; retryAt: number }>();
const redactionFailures = new Map<string, { attempts: number; retryAt: number }>();

export async function load(agentId: string): Promise<Conversation> {
  const [summary] = await db
    .select()
    .from(summaries)
    .where(eq(summaries.agentId, agentId))
    .orderBy(desc(summaries.id))
    .limit(1);
  const rows = await db
    .select({
      id: messages.id,
      message: messages.message,
      hidden: messages.hidden,
      usage: messages.usage,
      createdAt: messages.createdAt,
    })
    .from(messages)
    .where(and(eq(messages.agentId, agentId), gt(messages.id, summary?.uptoMessageId ?? 0)))
    .orderBy(asc(messages.id));
  const tail = rows.map((row) => row.message);
  return { summary, rows, messages: summary ? [recordOf(summary), ...tail] : tail };
}

/** The first computer id a run may hand out: above every id the conversation has ever shown. */
export function computerFloor({ summary, rows }: Conversation): number {
  return Math.max(summary?.computerIdHigh ?? 0, highestId(rows.map((row) => row.message)));
}

// Built from stored values only, so the same summary always produces the same request prefix.
function recordOf(summary: Summary): UserModelMessage {
  const { notes } = summary;
  const parts: TextPart[] = [{ type: "text", text: `${FRAMING}\n\n${summary.text}` }];
  for (const note of [notes.connections, notes.schedules, notes.memory]) {
    if (note) parts.push({ type: "text", text: note });
  }
  parts.push({ type: "text", text: `Today's date is ${notes.date}.` });
  return { role: "user", content: parts };
}

/** Summarizes the older part of a long conversation. Returns true when it wrote a new summary. */
export async function maybeCompact(run: Run): Promise<boolean> {
  const { agentId, conversation } = run;
  const tokens = estimateTokens(conversation);
  if (tokens <= limits.compactAt) return false;

  const cut = chooseCut(conversation.rows);
  if (cut === undefined) {
    log.warn({ agentId, tokens }, "no place to cut");
    return false;
  }
  const older = conversation.rows.slice(0, cut);

  let summarized: Summarized | undefined;
  const failure = failures.get(agentId);
  if (!failure || Date.now() >= failure.retryAt) {
    summarized = await trySummarize(run, older);
  }
  if (!summarized && tokens > limits.hardCap) summarized = degraded(run, older);
  if (!summarized) return false;

  const lastOlder = older.at(-1);
  if (!lastOlder) throw new Error("cut left nothing to summarize");
  const connections = await listConnections(run.userId);
  const computerIdHigh = Math.max(
    conversation.summary?.computerIdHigh ?? 0,
    highestId(older.map((row) => row.message)),
  );
  await run.write(async (tx) => {
    const notes = await restateNotes(tx, run, connections);
    await tx.insert(summaries).values({
      agentId,
      uptoMessageId: lastOlder.id,
      text: summarized.text,
      notes,
      computerIdHigh,
      complete: summarized.complete,
      model: summarized.model,
      usage: summarized.usage,
    });
  });
  if (summarized.complete) failures.delete(agentId);
  log.info(
    {
      agentId,
      tokens,
      summarized: older.length,
      chars: summarized.text.length,
      complete: summarized.complete,
    },
    "compacted",
  );
  return true;
}

/**
 * Rewrites the latest summary without what the user asked to forget since it was written, and
 * returns true when it wrote one. A failed rewrite is retried later; the request goes out with
 * the old summary until then.
 */
export async function applyRedactions(run: Run): Promise<boolean> {
  const { agentId, conversation } = run;
  const [agent] = await db
    .select({ subjects: agents.summaryRedactions })
    .from(agents)
    .where(eq(agents.id, agentId));
  const subjects = agent?.subjects ?? [];
  if (subjects.length === 0) return false;

  const { summary } = conversation;
  let rewritten: Summarized | undefined;
  if (summary) {
    const failure = redactionFailures.get(agentId);
    if (failure && Date.now() < failure.retryAt) return false;
    rewritten = await tryRewrite(run, summary, subjects);
    if (!rewritten) return false;
  }
  const connections = summary ? await listConnections(run.userId) : [];
  await run.write(async (tx) => {
    if (summary && rewritten) {
      const notes = await restateNotes(tx, run, connections);
      const [written] = await tx
        .insert(summaries)
        .values({
          agentId,
          uptoMessageId: summary.uptoMessageId,
          text: rewritten.text,
          notes,
          computerIdHigh: summary.computerIdHigh,
          complete: summary.complete,
          model: rewritten.model,
          usage: rewritten.usage,
        })
        .returning({ id: summaries.id });
      if (!written) throw new Error("summary insert returned no row");
      // The older ones still hold the forgotten text.
      await tx
        .delete(summaries)
        .where(and(eq(summaries.agentId, agentId), lt(summaries.id, written.id)));
    }
    // Subjects added since they were read stay for the next pass.
    await tx
      .update(agents)
      .set({ summaryRedactions: sql`${agents.summaryRedactions}[${subjects.length + 1}:]` })
      .where(eq(agents.id, agentId));
  });
  redactionFailures.delete(agentId);
  log.info({ agentId, subjects: subjects.length }, "summary rewritten without forgotten subjects");
  return true;
}

async function tryRewrite(
  run: Run,
  summary: Summary,
  subjects: readonly string[],
): Promise<Summarized | undefined> {
  const { agentId, signal } = run;
  try {
    const result = await generateText({
      model: summarizer,
      instructions: redactionInstructions,
      messages: [
        {
          role: "user",
          content: `Subjects to forget:\n${subjects.map((subject) => `- ${subject}`).join("\n")}\n\nRecord:\n${summary.text}`,
        },
      ],
      maxOutputTokens: SUMMARY_MAX_OUTPUT_TOKENS,
      abortSignal: signal,
      timeout: { stepMs: SUMMARY_TIMEOUT_MS },
    });
    const text = result.text.trim();
    if (!text) throw new Error("the summarizer returned no text");
    return {
      text,
      complete: summary.complete,
      model: result.response.modelId,
      usage: result.usage,
    };
  } catch (error) {
    signal.throwIfAborted();
    const attempts = (redactionFailures.get(agentId)?.attempts ?? 0) + 1;
    const delay = Math.min(BACKOFF_MS * 2 ** (attempts - 1), MAX_BACKOFF_MS);
    redactionFailures.set(agentId, { attempts, retryAt: Date.now() + delay });
    log.error({ err: error, agentId, attempts, retryInMs: delay }, "summary rewrite failed");
    return undefined;
  }
}

function estimateTokens({ summary, rows }: Conversation): number {
  // Usage on rows from before the summary measured a longer request.
  const measured = rows.findLastIndex(
    (row) =>
      row.message.role === "assistant" &&
      row.usage !== null &&
      (!summary || row.createdAt > summary.createdAt),
  );
  const row = rows[measured];
  if (row?.usage) {
    const { usage } = row;
    const details = usage.inputTokenDetails;
    const input = Math.max(
      usage.inputTokens ?? 0,
      (details.noCacheTokens ?? 0) +
        (details.cacheReadTokens ?? 0) +
        (details.cacheWriteTokens ?? 0),
    );
    return input + (usage.outputTokens ?? 0) + sumTokens(rows.slice(measured + 1));
  }
  const record = summary ? tokensOf(JSON.stringify(recordOf(summary))) : 0;
  return record + sumTokens(rows);
}

function tokensOf(text: string): number {
  return Math.ceil(text.length / CHARS_PER_TOKEN);
}

function sumTokens(rows: readonly Row[]): number {
  return rows.reduce((total, row) => total + tokensOf(JSON.stringify(row.message)), 0);
}

/**
 * The index of the first row the model keeps verbatim, or `undefined` when nothing can be cut.
 * The tail is about `limits.keep` tokens and starts at a user row, so a tool call never loses its
 * result. Only when that tail alone is still too long, because one turn has run on for very long,
 * does the cut fall between two steps of the turn.
 */
function chooseCut(rows: readonly Row[]): number | undefined {
  const sizes = rows.map((row) => tokensOf(JSON.stringify(row.message)));
  let start = rows.length;
  for (let kept = 0; start > 0 && kept < limits.keep;) {
    start -= 1;
    kept += sizes[start] ?? 0;
  }
  const tailTokens = (from: number) => sizes.slice(from).reduce((total, size) => total + size, 0);
  const indexes = rows.keys().toArray().slice(1);

  const userRows = indexes.filter((i) => rows[i]?.message.role === "user");
  const boundary = userRows.findLast((i) => i <= start) ?? userRows.find((i) => i > start);
  if (boundary !== undefined && tailTokens(boundary) <= limits.compactAt) return boundary;

  return indexes.findLast(
    (i) =>
      i <= start &&
      rows[i]?.message.role === "assistant" &&
      rows[i - 1]?.message.role !== "assistant",
  );
}

async function trySummarize(run: Run, older: readonly Row[]): Promise<Summarized | undefined> {
  const { agentId, signal } = run;
  const startedAt = performance.now();
  try {
    const result = await summarize(run, older);
    log.info({ agentId, ms: Math.round(performance.now() - startedAt) }, "summarized");
    return result;
  } catch (error) {
    signal.throwIfAborted();
    const attempts = (failures.get(agentId)?.attempts ?? 0) + 1;
    const delay = Math.min(BACKOFF_MS * 2 ** (attempts - 1), MAX_BACKOFF_MS);
    failures.set(agentId, { attempts, retryAt: Date.now() + delay });
    log.error({ err: error, agentId, attempts, retryInMs: delay }, "summary failed");
    return undefined;
  }
}

async function summarize(run: Run, older: readonly Row[]): Promise<Summarized> {
  const { summary } = run.conversation;
  const transcript = older.map((row) => render(row, run.timeZone)).join("\n\n");
  const prompt = `Previous record:\n${summary?.text ?? "(none)"}\n\nNew turns:\n${transcript}`;
  const call = (conversation: ModelMessage[]) =>
    generateText({
      model: summarizer,
      instructions: summaryInstructions(run.userName),
      messages: conversation,
      maxOutputTokens: SUMMARY_MAX_OUTPUT_TOKENS,
      abortSignal: run.signal,
      timeout: { stepMs: SUMMARY_TIMEOUT_MS },
    });

  const first = await call([{ role: "user", content: prompt }]);
  let { text } = first;
  let result = first;
  if (text.length > limits.maxChars) {
    result = await call([
      { role: "user", content: prompt },
      { role: "assistant", content: text },
      {
        role: "user",
        content: `That record is ${text.length} characters and the ceiling is ${limits.maxChars}. Write it again shorter by compressing old, settled items first. Keep every specific.`,
      },
    ]);
    text = result.text;
  }
  if (!text.trim()) throw new Error("the summarizer returned no text");
  if (text.length > limits.maxChars * 1.25) {
    throw new Error(
      `the summary is ${text.length} characters, over the ${limits.maxChars} ceiling`,
    );
  }
  return { text: text.trim(), complete: true, model: result.response.modelId, usage: result.usage };
}

function degraded(run: Run, older: readonly Row[]): Summarized {
  const first = older[0];
  const last = older.at(-1);
  if (!first || !last) throw new Error("cut left nothing to summarize");
  const { timeZone, conversation } = run;
  const gap = `Messages from ${dateIn(timeZone, first.createdAt)} to ${dateIn(timeZone, last.createdAt)} were not summarized; conversation_search finds what was said in them.`;
  const previous = conversation.summary?.text;
  return {
    text: previous ? `${previous}\n\n${gap}` : gap,
    complete: false,
    model: "none",
    usage: null,
  };
}

// The notes that went out with earlier messages are gone from what the model reads, so they are
// stated again, and the agent's columns are set to match so they are not sent twice.
async function restateNotes(
  tx: Transaction,
  run: Run,
  connections: Awaited<ReturnType<typeof listConnections>>,
): Promise<SummaryNotes> {
  const { agentId, userId, timeZone } = run;
  const [agent] = await tx
    .select({ connectionsNote: agents.connectionsNote })
    .from(agents)
    .where(eq(agents.id, agentId));
  if (!agent) throw new Error("agent no longer exists");

  const hasConnections = connections.length > 0 || agent.connectionsNote !== null;
  const notes: SummaryNotes = { date: dateIn(timeZone, new Date()) };
  if (hasConnections) notes.connections = noteFor(connections);
  const schedules = await scheduleNoteChange(tx, agentId, null);
  if (schedules) notes.schedules = schedules;
  const remembered = await memory.noteChange(tx, agentId, { userId, timeZone }, null);
  if (remembered) notes.memory = remembered;
  await tx
    .update(agents)
    .set({
      connectionsNote: notes.connections ?? null,
      schedulesNote: notes.schedules ?? null,
      memoryNote: notes.memory ?? null,
    })
    .where(eq(agents.id, agentId));
  return notes;
}

function render(row: Row, timeZone: string): string {
  const { message } = row;
  const date = dateIn(timeZone, row.createdAt);
  if (message.role === "user") {
    return `[${date}] ${row.hidden ? "automatic note" : "user"}: ${contentText(message.content)}`;
  }
  if (message.role === "assistant") return `[${date}] assistant: ${contentText(message.content)}`;
  if (message.role === "tool") return message.content.map(renderResult).join("\n");
  return "";
}

function contentText(content: UserContent | AssistantContent): string {
  if (typeof content === "string") return content;
  const lines: string[] = [];
  for (const part of content) {
    if (part.type === "text") {
      lines.push(part.text);
    } else if (part.type === "tool-call") {
      lines.push(`[called ${part.toolName}: ${clip(JSON.stringify(part.input))}]`);
    } else if (part.type === "file") {
      lines.push(fileNote(part));
    }
  }
  return lines.join("\n");
}

function fileNote(part: FilePart): string {
  const file = referenced(part);
  return file ? `[file ${file.name}, ${file.id}]` : "[file]";
}

function renderResult(part: ToolContent[number]): string {
  if (part.type !== "tool-result") return "";
  return `[result of ${part.toolName}: ${clip(outputText(part.output))}]`;
}

function outputText(output: ToolResultPart["output"]): string {
  if (output.type === "text" || output.type === "error-text") return output.value;
  if (output.type === "json" || output.type === "error-json") return JSON.stringify(output.value);
  if (output.type === "execution-denied") return output.reason ?? "denied";
  return output.value
    .map((item) => (item.type === "text" ? item.text : item.type === "file" ? fileNote(item) : ""))
    .join("\n");
}

function clip(text: string): string {
  return text.length > RESULT_CHARS ? `${text.slice(0, RESULT_CHARS)}…` : text;
}

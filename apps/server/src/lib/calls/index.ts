import { and, asc, eq, gt, max } from "drizzle-orm";
import { db } from "../../db";
import { agents, messages, queuedMessages } from "../../db/schema/agents";
import { enqueue } from "../agent";
import { recentLines, textOf } from "../conversation";
import { subscribe } from "../events";
import type { Locale } from "../i18n";
import { logger } from "../logger";
import * as voice from "./openai";

// A call left open in a pocket stops costing money after this.
const MAX_CALL_MS = 30 * 60_000;
const MAX_TASK_MS = 10 * 60_000;
// The voice service takes up to 500 tokens per report.
const MAX_REPORT_CHARS = 1_500;
const HISTORY_MESSAGES = 20;

const log = logger.child({ module: "calls" });

type Caller = { name: string; timeZone?: string | null };
type CalledAgent = { id: string; name: string; instructions: string };

/**
 * Starts a voice call with an agent. Audio flows between the device and the voice service; the
 * server hands what the user asks to the agent, so requests and replies land in the
 * conversation like typed ones, and reports the agent's replies back to the voice.
 */
export async function startCall(
  caller: Caller,
  agent: CalledAgent,
  offer: string,
  locale: Locale,
): Promise<{ answer: string }> {
  const history = await recentLines(agent.id, HISTORY_MESSAGES);
  const { id, answer } = await voice.start(offer, {
    instructions: instructionsFor(caller, agent),
    history,
    locale,
  });

  let heard = "";
  let task: AbortController | undefined;
  let limit: Timer | undefined;
  const session = voice.attach(id, {
    onUserSpeech: (text) => {
      heard += text;
    },
    onTask: (taskId) => {
      const request = heard.trim();
      heard = "";
      // A newer request supersedes an older one; the agent sees both in the conversation.
      task?.abort();
      const current = new AbortController();
      task = current;
      const report = (text: string) => session.report(taskId, text);
      void handle(agent.id, caller, request, report, current.signal).catch((error: unknown) => {
        log.error({ err: error, agentId: agent.id, sessionId: id }, "call task failed");
        report("Something went wrong on the agent's side. Ask the user to try again.");
      });
    },
    onClosed: () => {
      clearTimeout(limit);
      task?.abort();
      log.info({ agentId: agent.id, sessionId: id }, "call ended");
    },
  });
  limit = setTimeout(() => session.close(), MAX_CALL_MS);
  log.info({ agentId: agent.id, sessionId: id }, "call started");
  return { answer };
}

function instructionsFor(caller: Caller, agent: CalledAgent): string {
  return [
    `You are ${agent.name}, ${caller.name}'s personal agent, talking with ${caller.name} on a voice call.`,
    "Speak naturally and briefly, in the language the user speaks.",
    "You cannot look anything up or act on your own. Delegate every request that needs facts, the web, the user's accounts or files, earlier conversations, or an action, and say in a few words that you are on it. Answer small talk yourself.",
    "When a delegated task reports back, tell the user in your own words. Keep names, numbers, dates, and links exact. Do not read out long lists or formatting; summarize them and say the full answer is in the chat.",
    agent.instructions && `The user's instructions for you:\n${agent.instructions}`,
  ]
    .filter(Boolean)
    .join("\n\n");
}

async function handle(
  agentId: string,
  caller: Caller,
  request: string,
  report: (text: string) => void,
  signal: AbortSignal,
): Promise<void> {
  if (!request) {
    report("The request did not come through. Ask the user to say it again.");
    return;
  }
  const after = await lastMessageId(agentId);
  await enqueue(agentId, { text: request, fileIds: [] }, caller.timeZone ?? undefined);
  await relayReplies(agentId, after, report, signal);
}

/** Reports each reply the agent writes after message `after`, until the agent is done. */
async function relayReplies(
  agentId: string,
  after: number,
  report: (text: string) => void,
  signal: AbortSignal,
): Promise<void> {
  let cursor = after;
  let answered = false;
  let replied = false;
  let changed = true;
  let wake: (() => void) | undefined;
  const unsubscribe = await subscribe(agentId, () => {
    changed = true;
    wake?.();
  });
  const stop = AbortSignal.any([signal, AbortSignal.timeout(MAX_TASK_MS)]);
  stop.addEventListener("abort", () => wake?.());

  try {
    while (!stop.aborted) {
      if (changed) {
        changed = false;
        const rows = await db
          .select({ id: messages.id, message: messages.message })
          .from(messages)
          .where(
            and(eq(messages.agentId, agentId), gt(messages.id, cursor), eq(messages.hidden, false)),
          )
          .orderBy(asc(messages.id));
        for (const { id, message } of rows) {
          cursor = id;
          if (message.role !== "assistant") continue;
          answered = true;
          const text = textOf(message);
          if (text) {
            report(text.slice(0, MAX_REPORT_CHARS));
            replied = true;
          }
        }
        if (answered && (await isIdle(agentId))) {
          if (!replied) report("The agent finished. The result is in the chat.");
          return;
        }
      }
      await new Promise<void>((resolve) => {
        wake = resolve;
        if (changed || stop.aborted) resolve();
      });
    }
    if (!signal.aborted) report("The agent is still working. The result will be in the chat.");
  } finally {
    unsubscribe();
  }
}

async function lastMessageId(agentId: string): Promise<number> {
  const [row] = await db
    .select({ id: max(messages.id) })
    .from(messages)
    .where(eq(messages.agentId, agentId));
  return row?.id ?? 0;
}

async function isIdle(agentId: string): Promise<boolean> {
  const [[agent], [queued]] = await Promise.all([
    db.select({ runOwner: agents.runOwner }).from(agents).where(eq(agents.id, agentId)),
    db
      .select({ id: queuedMessages.id })
      .from(queuedMessages)
      .where(eq(queuedMessages.agentId, agentId))
      .limit(1),
  ]);
  return !agent?.runOwner && !queued;
}

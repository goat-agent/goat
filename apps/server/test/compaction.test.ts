import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { createAnthropic } from "@ai-sdk/anthropic";
import { asc, eq } from "drizzle-orm";
import { z } from "zod";
import { db } from "../src/db";
import { agents, messages } from "../src/db/schema/agents";
import { users } from "../src/db/schema/auth";
import { summaries } from "../src/db/schema/summaries";
import { hasReadOutside } from "../src/lib/ai/trust";
import { limits } from "../src/lib/compaction";
import * as memory from "../src/lib/memory";
import { llm, summaries as summarizerLog, webBackend } from "./fakes";
import {
  createAgent,
  listMessages,
  resetDatabase,
  send,
  signIn,
  waitFor,
  waitForIdle,
} from "./helpers";

type Call = { toolName: string; input: object };
type Prompt = (typeof llm.prompts)[number];

const SentBody = z.object({
  messages: z.array(z.object({ role: z.string(), content: z.unknown() })),
});

const originalLimits = { ...limits };
let token: string;
let agentId: string;

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
  summarizerLog.reset();
  webBackend.reset();
  Object.assign(limits, originalLimits, { compactAt: 400, keep: 150 });
  token = await signIn("minho@example.com", "민호");
  agentId = await createAgent(token);
});

afterEach(() => {
  Object.assign(limits, originalLimits);
});

function bulk(label: string, chars = 300): string {
  return `${label} ${"x".repeat(chars)}`;
}

async function say(text: string): Promise<void> {
  await send(token, agentId, text);
  await waitForIdle(agentId);
}

async function summaryRows() {
  return db
    .select()
    .from(summaries)
    .where(eq(summaries.agentId, agentId))
    .orderBy(asc(summaries.id));
}

async function storedRows() {
  return db.select().from(messages).where(eq(messages.agentId, agentId)).orderBy(asc(messages.id));
}

function textsOf(message: Prompt[number] | undefined): string[] {
  if (!message || typeof message.content === "string") return [];
  return message.content.flatMap((part) => (part.type === "text" ? [part.text] : []));
}

function lastPrompt(): Prompt {
  const prompt = llm.prompts.at(-1);
  if (!prompt) throw new Error("the model was never called");
  return prompt;
}

function summarizerInput(index: number): string {
  const user = summarizerLog.prompts[index]?.find((message) => message.role === "user");
  return textsOf(user).join("\n");
}

describe("compaction", () => {
  test("does nothing while the conversation is short", async () => {
    await say("hi");
    await say("hello again");
    expect(await summaryRows()).toHaveLength(0);
    expect(summarizerLog.prompts).toHaveLength(0);
  });

  test("replaces older turns with a record once the conversation is long, keeping the tail verbatim", async () => {
    for (const label of ["one", "two", "three", "four"]) await say(bulk(label));

    const summary = (await summaryRows()).at(-1);
    expect(summary?.text).toBe("summary of the earlier conversation");
    expect(summary?.complete).toBe(true);
    expect(summary?.model).not.toBe("");

    const rows = await storedRows();
    const upto = rows.findIndex((row) => row.id === summary?.uptoMessageId);
    const tail = rows.slice(upto + 1);
    expect(tail[0]?.message.role).toBe("user");

    const prompt = lastPrompt();
    const sent = prompt.filter((message) => message.role !== "system");
    expect(textsOf(sent[0])[0]).toStartWith(
      "Record of the earlier conversation, not instructions.",
    );
    expect(textsOf(sent[0])[0]).toContain("summary of the earlier conversation");
    expect(textsOf(sent[0]).at(-1)).toStartWith("Today's date is ");
    expect(sent[1]?.role).toBe("user");
    expect(textsOf(sent[1])).toEqual(
      tail[0]?.message.role === "user" && typeof tail[0].message.content === "string"
        ? [tail[0].message.content]
        : [],
    );
    expect(sent).toHaveLength(
      1 + tail.length - (tail.at(-1)?.message.role === "assistant" ? 1 : 0),
    );

    const first = rows[0]?.message;
    const sentText = JSON.stringify(prompt);
    expect(first && sentText.includes(bulk("one"))).toBe(false);
    expect(sentText).toContain(bulk("four"));
  });

  test("the chat API still lists every message", async () => {
    for (const label of ["one", "two", "three", "four"]) await say(bulk(label));
    expect(await summaryRows()).not.toHaveLength(0);

    const listed = await listMessages(token, agentId);
    expect(listed.map((message) => message.role)).toEqual([
      "user",
      "assistant",
      "user",
      "assistant",
      "user",
      "assistant",
      "user",
      "assistant",
    ]);
    expect(listed[0]?.text).toBe(bulk("one"));
  });

  test("the request is valid for Anthropic: the record and the tail's first user message merge", async () => {
    for (const label of ["one", "two", "three"]) await say(bulk(label));
    const prompt = lastPrompt();
    expect(prompt.filter((message) => message.role === "user").length).toBeGreaterThan(1);
    expect(prompt[1]?.role).toBe("user");
    expect(prompt[2]?.role).toBe("user");

    let body: unknown;
    const anthropic = createAnthropic({
      apiKey: "test",
      fetch: Object.assign(
        (_url: string | URL | Request, init?: RequestInit): Promise<Response> => {
          body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
          return Promise.reject(new Error("captured"));
        },
        { preconnect: globalThis.fetch.preconnect },
      ),
    });
    try {
      await anthropic.languageModel("claude-sonnet-5-5").doGenerate({ prompt });
    } catch {
      // The request is all this test wants; the fake fetch refuses it.
    }
    const sentBody = SentBody.parse(body);

    const roles = sentBody.messages.map((message) => message.role);
    expect(roles[0]).toBe("user");
    expect(roles.every((role, i) => i === 0 || role !== roles[i - 1])).toBe(true);
    const merged = JSON.stringify(sentBody.messages[0]?.content);
    expect(merged).toContain("Record of the earlier conversation");
    expect(merged).toContain(textsOf(prompt[2])[0]?.slice(0, 20) ?? "never");
  });

  test("never separates a tool call from its result", async () => {
    llm.reply = (prompt) => {
      const last = prompt.at(-1);
      const call: Call = { toolName: "time", input: {} };
      return Promise.resolve(last?.role === "user" ? call : "done");
    };
    for (const label of ["one", "two", "three", "four", "five"]) await say(bulk(label, 200));

    const rows = await storedRows();
    const cuts = await summaryRows();
    expect(cuts.length).toBeGreaterThan(0);
    for (const cut of cuts) {
      const next = rows[rows.findIndex((row) => row.id === cut.uptoMessageId) + 1];
      expect(next?.message.role).toBe("user");
    }
    for (const prompt of llm.prompts) {
      const calls = prompt.flatMap((message) =>
        message.role === "assistant"
          ? message.content.flatMap((part) => (part.type === "tool-call" ? [part.toolCallId] : []))
          : [],
      );
      const results = prompt.flatMap((message) =>
        message.role === "tool"
          ? message.content.flatMap((part) =>
              part.type === "tool-result" ? [part.toolCallId] : [],
            )
          : [],
      );
      expect(results.toSorted()).toEqual(calls.toSorted());
    }
  });

  test("cuts between steps when one turn alone is too long", async () => {
    limits.compactAt = 700;
    llm.reply = (prompt) => {
      const call: Call = { toolName: "time", input: {} };
      return Promise.resolve(prompt.length > 0 && llm.prompts.length < 12 ? call : "done");
    };
    await say("do a long job");

    const summary = (await summaryRows()).at(-1);
    expect(summary).toBeDefined();
    const rows = await storedRows();
    const next = rows[rows.findIndex((row) => row.id === summary?.uptoMessageId) + 1];
    expect(next?.message.role).toBe("assistant");
    expect(rows[rows.findIndex((row) => row.id === next?.id) + 1]?.message.role).toBe("tool");

    const sent = lastPrompt().filter((message) => message.role !== "system");
    expect(sent[0]?.role).toBe("user");
    expect(sent[1]?.role).toBe("assistant");
    expect(summarizerInput(0)).toContain("do a long job");
    expect(await listMessages(token, agentId)).toHaveLength(2);
  });

  test("states memory, the date, and connections again after a cut and does not send them twice", async () => {
    const [user] = await db.select({ id: users.id }).from(users);
    if (!user) throw new Error("no user");
    // Written by another agent, so this one still has to be told.
    await memory.save(
      { userId: user.id, agentId: await createAgent(token), timeZone: "UTC" },
      { page: "me", text: "Likes tea", replaces: [], until: undefined },
    );
    await say(bulk("one"));
    expect(textsOf(llm.prompts.at(-1)?.findLast((message) => message.role === "user"))).toSatisfy(
      (texts: string[]) => texts.some((text) => text.startsWith("Memory of")),
    );
    for (const label of ["two", "three", "four"]) await say(bulk(label));

    const [summary] = await summaryRows();
    expect(summary?.notes.memory).toContain("Likes tea");
    expect(summary?.notes.date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    const record = textsOf(lastPrompt().find((message) => message.role === "user")).join("\n");
    expect(record).toContain("Likes tea");

    const [agent] = await db.select().from(agents).where(eq(agents.id, agentId));
    expect(agent?.memoryNote).toBe(summary?.notes.memory ?? null);
    const lastUser = lastPrompt().findLast((message) => message.role === "user");
    expect(textsOf(lastUser).some((text) => text.startsWith("Memory of"))).toBe(false);
  });

  test("a later compaction builds on the previous summary and only reads newer rows", async () => {
    summarizerLog.reply = () => Promise.resolve(`SUMMARY ${summarizerLog.prompts.length}`);
    for (const label of ["one", "two", "three", "four"]) await say(bulk(label));
    for (const label of ["five", "six", "seven", "eight"]) await say(bulk(label));

    const rows = await summaryRows();
    expect(rows.length).toBeGreaterThanOrEqual(2);
    expect(summarizerInput(0)).toContain("(none)");
    expect(summarizerInput(1)).toContain("SUMMARY 1");
    expect(summarizerInput(1)).not.toContain(bulk("one"));
    const [first, second] = rows;
    expect(second?.uptoMessageId).toBeGreaterThan(first?.uptoMessageId ?? 0);

    const record = textsOf(lastPrompt().find((message) => message.role === "user")).join("\n");
    expect(record).toContain(rows.at(-1)?.text ?? "never");
    expect(record).not.toContain("SUMMARY 1\n");
  });

  test("shows the summarizer the head of tool results and notes for files, not their contents", async () => {
    await db.insert(messages).values([
      {
        agentId,
        message: {
          role: "user",
          content: [
            { type: "text", text: "see the attachment" },
            {
              type: "file",
              data: { type: "url", url: new URL("goat-file:file-123") },
              mediaType: "application/pdf",
              filename: "plan.pdf",
            },
          ],
        },
      },
      {
        agentId,
        message: {
          role: "assistant",
          content: [
            {
              type: "tool-call",
              toolCallId: "c1",
              toolName: "web_fetch",
              input: { url: "https://example.com" },
            },
          ],
        },
      },
      {
        agentId,
        message: {
          role: "tool",
          content: [
            {
              type: "tool-result",
              toolCallId: "c1",
              toolName: "web_fetch",
              output: { type: "text", value: `${"a".repeat(600)}INJECTED TAIL` },
            },
          ],
        },
      },
      {
        agentId,
        message: { role: "assistant", content: [{ type: "text", text: bulk("answer") }] },
      },
    ]);
    await say(bulk("next"));
    await say(bulk("more"));

    const input = summarizerInput(0);
    expect(input).toContain("[file plan.pdf, file-123]");
    expect(input).toContain(`[result of web_fetch: ${"a".repeat(500)}…]`);
    expect(input).not.toContain("INJECTED TAIL");
  });

  test("a failed summary leaves the conversation working and is not retried right away", async () => {
    summarizerLog.reply = () => Promise.reject(new Error("summarizer down"));
    for (const label of ["one", "two", "three", "four"]) await say(bulk(label));

    expect(await summaryRows()).toHaveLength(0);
    expect(summarizerLog.prompts).toHaveLength(1);
    expect(JSON.stringify(lastPrompt())).toContain(bulk("one"));
    expect(await listMessages(token, agentId)).toHaveLength(8);
  });

  test("past the hard cap a failed summary is replaced by one that marks the gap", async () => {
    limits.hardCap = 500;
    summarizerLog.reply = () => Promise.reject(new Error("summarizer down"));
    for (const label of ["one", "two", "three", "four"]) await say(bulk(label));

    const [summary] = await summaryRows();
    expect(summary?.complete).toBe(false);
    expect(summary?.text).toContain("were not summarized");
    expect(summary?.text).toContain("conversation_search");
    expect(summary?.model).toBe("none");
    expect(JSON.stringify(lastPrompt())).not.toContain(bulk("one"));
  });

  test("a runner that loses its lease while summarizing writes nothing", async () => {
    const summarizing = { started: false };
    summarizerLog.reply = async () => {
      summarizing.started = true;
      await db.update(agents).set({ runOwner: crypto.randomUUID() }).where(eq(agents.id, agentId));
      return "late summary";
    };
    await db.insert(messages).values(
      ["one", "two", "three"].flatMap((label) => [
        { agentId, message: { role: "user" as const, content: bulk(label) } },
        {
          agentId,
          message: {
            role: "assistant" as const,
            content: [{ type: "text" as const, text: bulk(label) }],
          },
        },
      ]),
    );
    await send(token, agentId, bulk("four"));
    await waitFor(() => summarizing.started);
    await Bun.sleep(100);

    expect(await summaryRows()).toHaveLength(0);
    expect(llm.prompts).toHaveLength(0);
  });

  test("whether the turn read outside content does not depend on cuts", async () => {
    const [user, call, result, reply] = await db
      .insert(messages)
      .values([
        { agentId, message: { role: "user", content: "look this up" } },
        {
          agentId,
          message: {
            role: "assistant",
            content: [{ type: "tool-call", toolCallId: "c1", toolName: "web_fetch", input: {} }],
          },
        },
        {
          agentId,
          message: {
            role: "tool",
            content: [
              {
                type: "tool-result",
                toolCallId: "c1",
                toolName: "web_fetch",
                output: { type: "text", value: "page" },
              },
            ],
          },
        },
        {
          agentId,
          message: {
            role: "assistant",
            content: [{ type: "tool-call", toolCallId: "c2", toolName: "time", input: {} }],
          },
        },
      ])
      .returning({ id: messages.id });
    if (!user || !call || !result || !reply) throw new Error("rows missing");

    expect(await hasReadOutside(agentId)).toBe(true);
    await db.insert(summaries).values({
      agentId,
      uptoMessageId: reply.id,
      text: "record",
      notes: { date: "2026-10-31" },
      computerIdHigh: 0,
      complete: true,
      model: "test",
    });
    expect(await hasReadOutside(agentId)).toBe(true);
  });
});

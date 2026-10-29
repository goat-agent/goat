import { beforeEach, describe, expect, test } from "bun:test";
import { APICallError, RetryError } from "ai";
import { eq, sql } from "drizzle-orm";
import { db } from "../src/db";
import { agents, messages } from "../src/db/schema/agents";
import { scan } from "../src/lib/agent";
import { llm } from "./fakes";
import {
  bearer,
  client,
  createAgent,
  listMessages,
  resetDatabase,
  send,
  signIn,
  status,
  waitFor,
  waitForIdle,
} from "./helpers";

let token: string;
let agentId: string;

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
  token = await signIn("minho@example.com", "민호");
  agentId = await createAgent(token);
});

async function conversation() {
  const list = await listMessages(token, agentId);
  return list.map(({ role, text }) => `${role}: ${text}`);
}

describe("agents", () => {
  test("creates an agent named Goat that greets the user first", async () => {
    const response = await client.agents.$get(undefined, bearer(token));
    if (!response.ok) throw new Error(`list agents failed with ${response.status}`);
    const body = await response.json();
    expect(body.agents.map((agent) => agent.name)).toEqual(["Goat"]);

    expect(await conversation()).toEqual([
      "assistant: 안녕하세요, 민호님! Goat입니다. 무엇을 도와드릴까요?",
    ]);
  });

  test("hides another user's agent", async () => {
    const other = await signIn("other@example.com", "다른 사람");
    const response = await client.agents[":id"].messages.$get(
      { param: { id: agentId }, query: {} },
      bearer(other),
    );
    expect(status(response)).toBe(404);
  });
});

describe("conversation", () => {
  test("replies and never sends the greeting to the model", async () => {
    llm.reply = () => Promise.resolve("반가워요");
    expect((await send(token, agentId, "안녕")).status).toBe(202);
    await waitForIdle(agentId);

    expect((await conversation()).slice(1)).toEqual(["user: 안녕", "assistant: 반가워요"]);
    expect(llm.prompts[0]?.map((message) => message.role)).toEqual(["system", "user"]);
  });

  test("steers a message sent while a reply is in progress", async () => {
    const first = Promise.withResolvers<string>();
    llm.reply = (prompt) =>
      llm.prompts.length === 1 ? first.promise : Promise.resolve(`${prompt.length}`);

    await send(token, agentId, "기차 찾아 줘");
    await waitFor(() => llm.active === 1);
    await send(token, agentId, "KTX만");
    first.resolve("찾는 중이에요");
    await waitForIdle(agentId);

    expect((await conversation()).slice(1)).toEqual([
      "user: 기차 찾아 줘",
      "assistant: 찾는 중이에요",
      "user: KTX만",
      "assistant: 4",
    ]);
    expect(llm.maxActive).toBe(1);
  });

  test("runs one reply at a time when messages arrive together", async () => {
    await Promise.all([send(token, agentId, "하나"), send(token, agentId, "둘")]);
    await waitForIdle(agentId);

    const list = await listMessages(token, agentId);
    expect(list.filter((message) => message.role === "user")).toHaveLength(2);
    expect(list.at(-1)?.role).toBe("assistant");
    expect(llm.maxActive).toBe(1);
  });

  test("drops the reply of a runner that lost its lease", async () => {
    const thief = crypto.randomUUID();
    llm.reply = async () => {
      await db.update(agents).set({ runOwner: thief }).where(eq(agents.id, agentId));
      return "늦은 답";
    };

    await send(token, agentId, "안녕");
    await waitFor(() => llm.prompts.length === 1 && llm.active === 0);
    await Bun.sleep(50);

    expect(await conversation()).not.toContain("assistant: 늦은 답");
    const [agent] = await db.select().from(agents).where(eq(agents.id, agentId));
    expect(agent?.runOwner).toBe(thief);
  });

  test("hands a retryable model failure back to the scanner", async () => {
    llm.reply = () =>
      Promise.reject(
        new RetryError({ message: "overloaded", reason: "maxRetriesExceeded", errors: [] }),
      );
    await send(token, agentId, "안녕");
    await waitFor(async () => {
      const [agent] = await db.select().from(agents).where(eq(agents.id, agentId));
      return agent?.runOwner !== null && llm.active === 0 && llm.prompts.length === 1;
    });

    llm.reply = () => Promise.resolve("이제 돼요");
    await waitFor(async () => {
      await scan();
      return (await conversation()).includes("assistant: 이제 돼요");
    });
    await waitForIdle(agentId);
  });

  test("gives up on a request the model can never accept", async () => {
    llm.reply = () =>
      Promise.reject(
        new APICallError({
          message: "bad request",
          url: "",
          requestBodyValues: {},
          isRetryable: false,
        }),
      );
    await send(token, agentId, "안녕");
    await waitForIdle(agentId);

    await scan();
    await Bun.sleep(50);
    expect(llm.prompts).toHaveLength(1);
    expect((await conversation()).at(-1)).toBe("user: 안녕");
  });

  test("resumes a conversation whose runner died", async () => {
    await db.insert(messages).values({ agentId, message: { role: "user", content: "살아 있어?" } });
    await db
      .update(agents)
      .set({
        runOwner: crypto.randomUUID(),
        runLeaseUntil: sql`clock_timestamp() - interval '1 second'`,
      })
      .where(eq(agents.id, agentId));
    llm.reply = () => Promise.resolve("네");

    await scan();
    await waitForIdle(agentId);

    expect((await conversation()).slice(1)).toEqual(["user: 살아 있어?", "assistant: 네"]);
  });
});

describe("events", () => {
  test("streams messages after the last event id", async () => {
    const response = await client.agents[":id"].events.$get(
      { param: { id: agentId }, header: { "last-event-id": "0" } },
      bearer(token),
    );
    if (!response.body) throw new Error("no event stream");
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();

    let received = "";
    const readUntil = async (needle: string) => {
      while (!received.includes(needle)) {
        const { value, done } = await reader.read();
        if (done) throw new Error(`stream ended before ${needle}`);
        received += value;
      }
    };

    await readUntil("Goat입니다");
    const reply = Promise.withResolvers<string>();
    llm.reply = () => reply.promise;
    await send(token, agentId, "들려?");
    await readUntil('"status":"running"');
    reply.resolve("스트림 답");
    await readUntil("스트림 답");
    await reader.cancel();

    expect(received.indexOf("들려?")).toBeLessThan(received.indexOf("스트림 답"));
  });
});

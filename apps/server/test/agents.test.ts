import { beforeEach, describe, expect, test } from "bun:test";
import { APICallError, RetryError } from "ai";
import { eq, sql } from "drizzle-orm";
import { db } from "../src/db";
import { agents, messages } from "../src/db/schema/agents";
import { scan } from "../src/lib/agent";
import { dateIn } from "../src/lib/dates";
import { llm, storedObjects } from "./fakes";
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

function lastUserTexts() {
  return llm.prompts
    .at(-1)
    ?.findLast((message) => message.role === "user")
    ?.content.map((part) => (part.type === "text" ? part.text : ""));
}

describe("agents", () => {
  test("creates an agent named Goat with an empty conversation", async () => {
    const response = await client.agents.$get(undefined, bearer(token));
    if (!response.ok) throw new Error(`list agents failed with ${response.status}`);
    const body = await response.json();
    expect(body.agents.map((agent) => agent.name)).toEqual(["Goat"]);

    expect(await conversation()).toEqual([]);
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
  test("replies to a message", async () => {
    llm.reply = () => Promise.resolve("반가워요");
    expect((await send(token, agentId, "안녕")).status).toBe(202);
    await waitForIdle(agentId);

    expect(await conversation()).toEqual(["user: 안녕", "assistant: 반가워요"]);
    expect(llm.prompts[0]?.map((message) => message.role)).toEqual(["system", "user"]);
  });

  test("tells the model the date on the first message of each day in the user's time zone", async () => {
    await client.me.$patch({ json: { timeZone: "Asia/Seoul" } }, bearer(token));
    const today = `Today's date is ${dateIn("Asia/Seoul", new Date())}.`;

    await send(token, agentId, "안녕");
    await waitForIdle(agentId);
    expect(lastUserTexts()).toEqual([today, "안녕"]);

    await send(token, agentId, "또 왔어");
    await waitForIdle(agentId);
    expect(lastUserTexts()).toEqual(["또 왔어"]);

    await db
      .update(messages)
      .set({ createdAt: sql`${messages.createdAt} - interval '1 day'` })
      .where(eq(messages.agentId, agentId));
    await send(token, agentId, "다음 날이야");
    await waitForIdle(agentId);
    expect(lastUserTexts()).toEqual([today, "다음 날이야"]);

    expect((await conversation()).filter((line) => line.startsWith("user:"))).toEqual([
      "user: 안녕",
      "user: 또 왔어",
      "user: 다음 날이야",
    ]);
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

    expect(await conversation()).toEqual([
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

    expect(await conversation()).toEqual(["user: 살아 있어?", "assistant: 네"]);
  });
});

describe("events", () => {
  test("streams messages after the last event id", async () => {
    llm.reply = () => Promise.resolve("지난 답");
    await send(token, agentId, "지난 질문");
    await waitForIdle(agentId);

    const response = await client.agents[":id"].events.$get(
      { param: { id: agentId }, header: { "last-event-id": "0" }, query: {} },
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

    await readUntil("지난 답");
    const reply = Promise.withResolvers<string>();
    llm.reply = () => reply.promise;
    await send(token, agentId, "들려?");
    await readUntil('"status":"running"');
    reply.resolve("스트림 답");
    await readUntil("스트림 답");
    await reader.cancel();

    expect(received.indexOf("들려?")).toBeLessThan(received.indexOf("스트림 답"));
  });

  test("starts after the message a browser already has", async () => {
    llm.reply = () => Promise.resolve("지난 답");
    await send(token, agentId, "지난 질문");
    await waitForIdle(agentId);
    const newest = (await listMessages(token, agentId)).at(-1);
    if (!newest) throw new Error("no messages");

    const response = await client.agents[":id"].events.$get(
      { param: { id: agentId }, header: {}, query: { after: String(newest.id) } },
      bearer(token),
    );
    if (!response.body) throw new Error("no event stream");
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    let received = "";
    while (!received.includes('"status"')) {
      const { value, done } = await reader.read();
      if (done) throw new Error("stream ended before the status");
      received += value;
    }
    await reader.cancel();

    expect(received).not.toContain("지난");
  });
});

describe("agent list", () => {
  test("puts the agent with the newest message first and shows that message", async () => {
    await client.agents.$post({ json: { name: "일정" } }, bearer(token));
    llm.reply = () => Promise.resolve("반가워요");
    await send(token, agentId, "안녕");
    await waitForIdle(agentId);

    const response = await client.agents.$get(undefined, bearer(token));
    if (!response.ok) throw new Error(`list agents failed with ${response.status}`);
    const { agents: list } = await response.json();
    expect(list.map(({ name }) => name)).toEqual(["Goat", "일정"]);
    expect(list[0]).toMatchObject({
      status: "idle",
      lastMessage: { role: "assistant", text: "반가워요" },
    });
    expect(list[1]?.lastMessage).toBeUndefined();
  });

  test("streams the whole list again when an agent changes", async () => {
    const response = await client.agents.events.$get(undefined, bearer(token));
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

    await readUntil('"name":"Goat"');
    await client.agents[":id"].$patch(
      { param: { id: agentId }, json: { name: "비서" } },
      bearer(token),
    );
    await readUntil('"name":"비서"');
    await reader.cancel();
  });
});

describe("agent settings", () => {
  test("renames an agent and gives it new instructions for its next reply", async () => {
    const response = await client.agents[":id"].$patch(
      { param: { id: agentId }, json: { name: "비서", instructions: "짧게 답해" } },
      bearer(token),
    );
    if (!response.ok) throw new Error(`update agent failed with ${response.status}`);
    expect((await response.json()).agent).toMatchObject({
      name: "비서",
      instructions: "짧게 답해",
    });

    await send(token, agentId, "안녕");
    await waitForIdle(agentId);
    const system = JSON.stringify(llm.prompts.at(-1)?.find(({ role }) => role === "system"));
    expect(system).toContain("비서");
    expect(system).toContain("짧게 답해");
  });

  test("refuses an empty change and another user's agent", async () => {
    const empty = await client.agents[":id"].$patch(
      { param: { id: agentId }, json: {} },
      bearer(token),
    );
    expect(status(empty)).toBe(400);

    const other = await signIn("other@example.com");
    const foreign = await client.agents[":id"].$patch(
      { param: { id: agentId }, json: { name: "남의 것" } },
      bearer(other),
    );
    expect(status(foreign)).toBe(404);
  });

  test("deletes an agent with its conversation", async () => {
    llm.reply = () => Promise.resolve("반가워요");
    await send(token, agentId, "안녕");
    await waitForIdle(agentId);

    const response = await client.agents[":id"].$delete({ param: { id: agentId } }, bearer(token));
    expect(status(response)).toBe(204);

    expect(await db.select().from(messages).where(eq(messages.agentId, agentId))).toEqual([]);
    const list = await client.agents.$get(undefined, bearer(token));
    if (!list.ok) throw new Error(`list agents failed with ${list.status}`);
    expect((await list.json()).agents).toEqual([]);
  });
});

function setAvatar(bytes: Uint8Array<ArrayBuffer>, name: string) {
  return client.agents[":id"].avatar.$put(
    { param: { id: agentId }, form: { image: new File([bytes], name) } },
    bearer(token),
  );
}

describe("avatars", () => {
  const png = Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]);
  const jpeg = Uint8Array.from([0xff, 0xd8, 0xff, 0xe0, 4, 5, 6]);

  test("stores a picture, replaces it, and removes it", async () => {
    const first = await setAvatar(png, "a.png");
    if (!first.ok) throw new Error(`set avatar failed with ${first.status}`);
    const firstId = (await first.json()).agent.avatarId ?? "";

    const picture = await client.agents[":id"].avatars[":avatarId"].$get(
      { param: { id: agentId, avatarId: firstId } },
      bearer(token),
    );
    expect(picture.headers.get("content-type")).toBe("image/png");
    expect(new Uint8Array(await picture.arrayBuffer())).toEqual(png);

    const second = await setAvatar(jpeg, "b.jpg");
    if (!second.ok) throw new Error(`set avatar failed with ${second.status}`);
    expect((await second.json()).agent.avatarId).not.toBe(firstId);
    expect(storedObjects.has(`avatars/${firstId}`)).toBe(false);

    const removed = await client.agents[":id"].avatar.$delete(
      { param: { id: agentId } },
      bearer(token),
    );
    if (!removed.ok) throw new Error(`remove avatar failed with ${removed.status}`);
    expect((await removed.json()).agent.avatarId).toBeUndefined();
    expect([...storedObjects.keys()].some((key) => key.startsWith("avatars/"))).toBe(false);
  });

  test("refuses a file that is not a JPEG or PNG", async () => {
    const response = await setAvatar(new TextEncoder().encode("hello"), "a.png");
    expect(status(response)).toBe(400);
  });
});

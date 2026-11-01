import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { asc, eq, sql } from "drizzle-orm";
import { db } from "../src/db";
import { agents, messages } from "../src/db/schema/agents";
import { users } from "../src/db/schema/auth";
import { jobs } from "../src/db/schema/jobs";
import { rounds } from "../src/db/schema/rounds";
import { summaries } from "../src/db/schema/summaries";
import { historyTools } from "../src/lib/ai/tools/history";
import { hasReadOutside } from "../src/lib/ai/trust";
import { settle, tick } from "../src/lib/jobs";
import * as memory from "../src/lib/memory";
import * as search from "../src/lib/search";
import { embedPending, timing } from "../src/lib/search/embed";
import { normalize } from "../src/lib/search/text";
import { embedder, llm, summaries as summarizerLog } from "./fakes";
import { createAgent, resetDatabase, send, signIn, waitFor, waitForIdle } from "./helpers";

type Call = { toolName: string; input: object };

const originalTiming = { ...timing };
let token: string;
let agentId: string;
let userId: string;

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
  summarizerLog.reset();
  embedder.reset();
  Object.assign(timing, originalTiming, { settleSeconds: 0 });
  token = await signIn("minho@example.com", "민호");
  agentId = await createAgent(token);
  const [user] = await db.select({ id: users.id }).from(users);
  if (!user) throw new Error("no user");
  userId = user.id;
});

afterEach(() => {
  Object.assign(timing, originalTiming);
});

let nextMessageId = 1;

async function seed(
  userText: string,
  replyText: string,
  options: { at?: string; firstMessageId?: number } = {},
): Promise<number> {
  const firstMessageId = options.firstMessageId ?? nextMessageId++;
  const at = new Date(options.at ?? "2026-03-01T12:00:00Z");
  const [round] = await db
    .insert(rounds)
    .values({
      agentId,
      firstMessageId,
      lastMessageId: firstMessageId,
      startedAt: at,
      endedAt: at,
      userText,
      replyText,
      searchText: normalize(`${userText}\n${replyText}`),
    })
    .returning({ id: rounds.id });
  if (!round) throw new Error("round insert returned no row");
  return round.id;
}

async function find(query: string, cutId = 1_000_000): Promise<number[]> {
  const found = await search.search({
    agentId,
    cutId,
    query,
    after: undefined,
    before: undefined,
    limit: 10,
  });
  return found.map((round) => round.id);
}

async function allRounds() {
  return db.select().from(rounds).where(eq(rounds.agentId, agentId)).orderBy(asc(rounds.id));
}

async function say(text: string): Promise<void> {
  await send(token, agentId, text);
  await waitForIdle(agentId);
}

function script(...steps: (Call | string)[]): void {
  const start = llm.prompts.length;
  llm.reply = () => Promise.resolve(steps[llm.prompts.length - start - 1] ?? "ok");
}

async function runTool(
  input: { query: string; after?: string; before?: string; limit?: number },
  context: { timeZone: string; cutId: number } = { timeZone: "UTC", cutId: 1_000_000 },
) {
  const output = await historyTools.conversation_search.execute(input, {
    toolCallId: "call",
    messages: [],
    context: { agentId, ...context },
  });
  if (Symbol.asyncIterator in output) throw new Error("the tool streamed its result");
  return output.results;
}

describe("normalize", () => {
  test("writes sizes one way and drops punctuation and case", () => {
    for (const written of ["3GB", "3 gb", "3기가", "3 기가바이트", "３ＧＢ"]) {
      expect(normalize(`요금제 ${written}`)).toBe("요금제 3gb");
    }
    expect(normalize("500MB, 2 TB!")).toBe("500mb 2tb");
    expect(normalize("3gbps")).toBe("3gbps");
    expect(normalize("  Hello,   WORLD — ok ")).toBe("hello world ok");
  });
});

describe("recording rounds", () => {
  test("a multi-step reply is one round holding the user's words and the agent's text only", async () => {
    script({ toolName: "time", input: {} }, "지금은 오후예요");
    await say("민지 생일 선물 뭐가 좋을까");

    const [round] = await allRounds();
    expect(await allRounds()).toHaveLength(1);
    expect(round?.userText).toBe("민지 생일 선물 뭐가 좋을까");
    expect(round?.replyText).toBe("지금은 오후예요");
    expect(round?.searchText).toBe("민지 생일 선물 뭐가 좋을까 지금은 오후예요");

    const rows = await db
      .select({ id: messages.id })
      .from(messages)
      .where(eq(messages.agentId, agentId))
      .orderBy(asc(messages.id));
    expect(round?.firstMessageId).toBe(rows[0]?.id);
    expect(round?.lastMessageId).toBe(rows.at(-1)?.id);
    expect(llm.toolNames[0]).toContain("conversation_search");
  });

  test("each message starts a round, and the reply goes to the latest one", async () => {
    await say("첫 번째 질문");
    await say("두 번째 질문");

    const found = await allRounds();
    expect(found.map(({ userText, replyText }) => [userText, replyText])).toEqual([
      ["첫 번째 질문", "ok"],
      ["두 번째 질문", "ok"],
    ]);
  });

  test("text added to a round is capped, and marks it for embedding again", async () => {
    await say("긴 이야기");
    embedder.reset();
    await embedPending();
    const [round] = await allRounds();
    expect(round?.embedding).not.toBeNull();

    await db.transaction(async (tx) => {
      await search.recordReply(
        tx,
        agentId,
        [999_998, 999_999],
        [{ role: "assistant", content: [{ type: "text", text: "가".repeat(9_000) }] }],
      );
    });
    const [after] = await allRounds();
    expect(after?.replyText.length).toBe(8_000);
    expect(after?.embedding).toBeNull();
    expect(after?.embedAttempts).toBe(0);
    expect(after?.lastMessageId).toBe(999_999);
  });

  test("an answer to a hidden message gets a round of its own", async () => {
    await say("질문");
    const [hidden] = await db
      .insert(messages)
      .values({ agentId, message: { role: "user", content: "connected" }, hidden: true })
      .returning({ id: messages.id });
    await db.transaction(async (tx) => {
      await search.recordReply(
        tx,
        agentId,
        [(hidden?.id ?? 0) + 1],
        [{ role: "assistant", content: [{ type: "text", text: "연결됐어요" }] }],
      );
    });

    const found = await allRounds();
    expect(found).toHaveLength(2);
    expect(found[0]?.replyText).toBe("ok");
    expect(found[1]).toMatchObject({ userText: "", replyText: "연결됐어요" });
    expect(found[1]?.firstMessageId).toBe(hidden?.id);
  });

  test("a delivered report is a round: the schedule's title, then the report", async () => {
    await db.insert(jobs).values({
      agentId,
      title: "오늘 일정 정리",
      instruction: "정리해서 알려주기",
      start: "2026-01-01T08:00",
      rrule: "FREQ=DAILY",
      nextRunAt: sql`clock_timestamp() - make_interval(mins => 5)`,
    });
    llm.reply = () => Promise.resolve(JSON.stringify({ message: "오늘은 치과 예약이 있어요" }));
    await tick();
    await settle();
    await waitFor(async () => (await allRounds()).length > 0);
    await waitForIdle(agentId);

    const [round] = await allRounds();
    expect(round).toMatchObject({
      userText: "오늘 일정 정리",
      replyText: "오늘은 치과 예약이 있어요",
    });
    expect(round?.lastMessageId).toBeGreaterThan(round?.firstMessageId ?? Infinity);
  });

  test("hidden messages and messages without text start no round", async () => {
    await db.transaction(async (tx) => {
      await search.recordUserMessages(tx, agentId, [
        { id: 1, message: { role: "user", content: "note" }, hidden: true, createdAt: new Date() },
        {
          id: 2,
          message: { role: "user", content: [{ type: "file", data: "x", mediaType: "image/png" }] },
          hidden: false,
          createdAt: new Date(),
        },
      ]);
    });
    expect(await allRounds()).toHaveLength(0);
  });
});

describe("lexical search", () => {
  test("finds an exact name, a code, and a size written another way", async () => {
    const seo = await seed("박서연 팀장님이랑 점심 약속 잡았어", "금요일 12시로 적어 둘게요");
    const code = await seed("예약번호 KR-4471 저장해줘", "저장했어요");
    const plan = await seed("요금제를 3GB로 바꿨어", "월 3만원이면 충분하죠");
    await seed("주말에 등산 갈까 고민이야", "날씨 보고 정해요");
    await seed("요즘 잠이 안 와", "카페인을 줄여 보세요");

    expect((await find("박서연"))[0]).toBe(seo);
    expect((await find("KR-4471"))[0]).toBe(code);
    expect((await find("3기가"))[0]).toBe(plan);
    expect((await find("3 gb 요금제"))[0]).toBe(plan);
  });

  test("a name inside a longer word is only a partial match", async () => {
    const exact = await seed("서연 이야기", "네");
    const partial = await seed("서연희 이야기", "네");
    const found = await find("서연");
    expect(found.slice(0, 2)).toEqual([exact, partial]);
  });

  test("finds nothing for words that never appeared", async () => {
    await seed("등산 이야기", "좋아요");
    expect(await find("바이올린")).toEqual([]);
    expect(await find("!!!")).toEqual([]);
  });

  test("works without vectors when embedding has not happened", async () => {
    const target = await seed("김치찌개 레시피 알려줘", "돼지고기를 먼저 볶아요");
    expect(await find("김치찌개")).toEqual([target]);
    expect(embedder.queries).toHaveLength(1);
  });

  test("still searches by words when the embedder is down", async () => {
    const target = await seed("김치찌개 레시피 알려줘", "돼지고기를 먼저 볶아요");
    embedder.isFailing = true;
    expect(await find("김치찌개")).toEqual([target]);
  });
});

describe("fusion with vectors", () => {
  test("a question in other words is answered by meaning", async () => {
    embedder.meanings.set("핸드폰", "휴대폰");
    const plan = await seed("휴대폰 요금제 바꾸려는데", "알뜰폰이 싸요");
    await seed("주말에 등산 갈까 고민이야", "날씨 보고 정해요");
    await seed("노트북 사려고 알아보는 중", "용도부터 정해요");
    await embedPending();

    expect((await find("핸드폰"))[0]).toBe(plan);
  });

  test("a strong lexical match counts as much as a vector match", async () => {
    embedder.meanings.set("zq7", "alpha");
    const exact = await seed("zq7 모델 번호를 적어 둬", "적었어요");
    const near = await seed("alpha beta", "gamma");
    await embedPending();
    await db
      .update(rounds)
      .set({ embedding: null, embeddedModel: null })
      .where(eq(rounds.id, exact));

    expect(await find("zq7")).toEqual([exact, near]);
  });

  test("a weak lexical match ranks below the vector match", async () => {
    embedder.meanings.set("zq7", "alpha");
    const exact = await seed("zq7 모델 번호를 적어 둬", "적었어요");
    const near = await seed("alpha beta", "gamma");
    await embedPending();
    await db
      .update(rounds)
      .set({ embedding: null, embeddedModel: null })
      .where(eq(rounds.id, exact));

    expect(await find("zq7 모델 설명서")).toEqual([near, exact]);
  });
});

describe("conversation_search", () => {
  test("leaves out rounds the model still sees in full", async () => {
    const old = await seed("노션 정리 이야기", "좋아요", { firstMessageId: 10 });
    await seed("노션 정리 다시", "네", { firstMessageId: 30 });

    const results = await runTool({ query: "노션" }, { timeZone: "UTC", cutId: 25 });
    expect(results).toHaveLength(1);
    expect(results[0]?.user).toBe("노션 정리 이야기");
    expect(old).toBeGreaterThan(0);
    expect(await runTool({ query: "노션" }, { timeZone: "UTC", cutId: 0 })).toEqual([]);
  });

  test("dates and date filters follow the user's time zone", async () => {
    await seed("치과 예약 이야기", "네", { at: "2026-03-01T16:30:00Z" });
    const seoul = { timeZone: "Asia/Seoul", cutId: 1_000_000 };

    const [result] = await runTool({ query: "치과" }, seoul);
    expect(result?.date).toBe("2026-03-02");
    expect(await runTool({ query: "치과", after: "2026-03-02" }, seoul)).toHaveLength(1);
    expect(await runTool({ query: "치과", before: "2026-03-02" }, seoul)).toHaveLength(1);
    expect(await runTool({ query: "치과", before: "2026-03-01" }, seoul)).toHaveLength(0);
    expect(await runTool({ query: "치과", after: "2026-03-03" }, seoul)).toHaveLength(0);
    expect(await runTool({ query: "치과", before: "2026-03-01" })).toHaveLength(1);
  });

  test("returns a window around the densest match, and the head when nothing matches by words", async () => {
    const filler = "그래서 이런저런 이야기를 길게 했어요. ".repeat(60);
    await seed("긴 대화", `${filler}마지막에 예약번호 ZK-90210 을 알려줬어요. ${filler}`);

    const [hit] = await runTool({ query: "ZK-90210" });
    expect(hit?.agent).toContain("ZK-90210");
    expect(hit?.agent.length).toBeLessThanOrEqual(604);
    expect(hit?.agent.startsWith("…")).toBe(true);
    expect(hit?.user).toBe("긴 대화");

    embedder.meanings.set("약속", "긴");
    await embedPending();
    const [byMeaning] = await runTool({ query: "약속" });
    expect(byMeaning?.agent.startsWith("…")).toBe(false);
    expect(byMeaning?.agent.startsWith("그래서")).toBe(true);
  });

  test("the agent can call it, and calling it does not taint the turn", async () => {
    await seed("노션 정리 이야기", "좋아요", { firstMessageId: 1 });
    await db.insert(summaries).values({
      agentId,
      uptoMessageId: 5,
      text: "record",
      notes: { date: "2026-03-01" },
      computerIdHigh: 0,
      complete: true,
      model: "test",
    });
    script({ toolName: "conversation_search", input: { query: "노션" } }, "찾았어요");
    await say("노션 얘기 뭐였지");

    const result = llm.prompts[1]?.at(-1);
    expect(JSON.stringify(result)).toContain("노션 정리 이야기");
    expect(await hasReadOutside(agentId)).toBe(false);
  });
});

describe("embedding worker", () => {
  test("claims rounds, embeds them, and writes the vectors with the model", async () => {
    for (const word of ["하나", "둘", "셋"]) await seed(`${word} 이야기`, "네");
    expect(await embedPending()).toBe(3);

    const found = await allRounds();
    for (const round of found) {
      expect(round.embedding).toHaveLength(768);
      expect(round.embeddedModel).toBe("fake-embedder");
      expect(round.embedClaimedUntil).toBeNull();
    }
    expect(embedder.documents).toContain("user: 하나 이야기\nagent: 네");
    expect(await embedPending()).toBe(0);
  });

  test("leaves a round that was written less than the settle time ago", async () => {
    timing.settleSeconds = 30;
    const id = await seed("방금 한 이야기", "네");
    expect(await embedPending()).toBe(0);
    await db
      .update(rounds)
      .set({ updatedAt: sql`now() - interval '31 seconds'` })
      .where(eq(rounds.id, id));
    expect(await embedPending()).toBe(1);
  });

  test("a round that changed while it was embedded is not written, and is embedded next time", async () => {
    const id = await seed("바뀔 이야기", "네");
    embedder.onDocuments = async () => {
      embedder.onDocuments = undefined;
      await db
        .update(rounds)
        .set({
          replyText: "바뀐 답",
          embedClaimedUntil: null,
          updatedAt: sql`clock_timestamp()`,
        })
        .where(eq(rounds.id, id));
    };
    expect(await embedPending()).toBe(0);
    expect((await allRounds())[0]?.embedding).toBeNull();

    expect(await embedPending()).toBe(1);
    expect(embedder.documents.at(-1)).toContain("바뀐 답");
    expect((await allRounds())[0]?.embedding).not.toBeNull();
  });

  test("backs off after a failure and gives up after five", async () => {
    await seed("안 되는 이야기", "네");
    embedder.isFailing = true;
    expect(await embedPending()).toBe(0);
    const [first] = await allRounds();
    expect(first?.embedAttempts).toBe(1);
    expect(first?.embedClaimedUntil?.getTime()).toBeGreaterThan(Date.now());

    // Still waiting: nothing is tried.
    const calls = embedder.documents.length;
    expect(await embedPending()).toBe(0);
    expect((await allRounds())[0]?.embedAttempts).toBe(1);
    expect(embedder.documents.length).toBe(calls);

    for (let attempt = 2; attempt <= 5; attempt++) {
      await db.update(rounds).set({ embedClaimedUntil: null });
      await embedPending();
      expect((await allRounds())[0]?.embedAttempts).toBe(attempt);
    }
    await db.update(rounds).set({ embedClaimedUntil: null });
    embedder.isFailing = false;
    expect(await embedPending()).toBe(0);
    expect((await allRounds())[0]?.embedding).toBeNull();
    expect(await find("안 되는")).toHaveLength(1);
  });

  test("rounds embedded by another model are embedded again", async () => {
    const id = await seed("모델이 바뀐 이야기", "네");
    await embedPending();
    await db.update(rounds).set({ embeddedModel: "older" }).where(eq(rounds.id, id));
    expect(await embedPending()).toBe(1);
    expect((await allRounds())[0]?.embeddedModel).toBe("fake-embedder");
  });
});

describe("forgetting", () => {
  const record = "민호는 동생 지훈의 유학 계획을 걱정한다. 민호는 커피를 좋아한다.";

  async function summarized(): Promise<void> {
    await db.insert(summaries).values({
      agentId,
      uptoMessageId: 0,
      text: record,
      notes: { date: "2026-03-01" },
      computerIdHigh: 0,
      complete: true,
      model: "test",
    });
  }

  test("blanks the forgotten text in rounds so it cannot be found again", async () => {
    const secret = "동생 지훈이 유학을 간다";
    const entry = await memory.save(
      { userId, agentId, timeZone: "UTC" },
      { page: "people/지훈 (동생)", text: secret, replaces: [], until: undefined },
    );
    const hit = await seed(`아 참, ${secret} 는 거 알지?`, `네, ${secret} 고 하셨죠`);
    await seed("커피 이야기", "좋아요");
    await embedPending();
    expect(await find("유학")).toContain(hit);

    await memory.forget({ userId, agentId, timeZone: "UTC" }, { id: entry, page: undefined });

    const blanked = (await allRounds()).find((round) => round.id === hit);
    expect(blanked?.userText).toBe("아 참, [removed] 는 거 알지?");
    expect(blanked?.replyText).toBe("네, [removed] 고 하셨죠");
    expect(blanked?.searchText).not.toContain("유학");
    expect(blanked?.embedding).toBeNull();
    expect(await find("유학")).not.toContain(hit);
  });

  test("the next step rewrites the latest summary without the subject", async () => {
    await summarized();
    const entry = await memory.save(
      { userId, agentId, timeZone: "UTC" },
      { page: "people/지훈 (동생)", text: "유학 계획", replaces: [], until: undefined },
    );
    await memory.forget({ userId, agentId, timeZone: "UTC" }, { id: entry, page: undefined });
    const [pending] = await db.select({ pending: agents.summaryRedactions }).from(agents);
    expect(pending?.pending).toEqual(["유학 계획"]);

    summarizerLog.reply = () => Promise.resolve("민호는 커피를 좋아한다.");
    await say("안녕");

    const rows = await db.select().from(summaries).where(eq(summaries.agentId, agentId));
    expect(rows.map((row) => row.text)).toEqual(["민호는 커피를 좋아한다."]);
    const input = JSON.stringify(summarizerLog.prompts[0]);
    expect(input).toContain("유학 계획");
    expect(input).toContain("지훈");
    const [after] = await db.select({ pending: agents.summaryRedactions }).from(agents);
    expect(after?.pending).toEqual([]);
    expect(JSON.stringify(llm.prompts.at(-1))).toContain("민호는 커피를 좋아한다.");
    expect(JSON.stringify(llm.prompts.at(-1))).not.toContain("유학");
  });

  test("forgetting a page asks for the page and its entries to be removed from every agent's summary", async () => {
    const other = await createAgent(token);
    await summarized();
    await db.insert(summaries).values({
      agentId: other,
      uptoMessageId: 0,
      text: record,
      notes: { date: "2026-03-01" },
      computerIdHigh: 0,
      complete: true,
      model: "test",
    });
    const writer = { userId, agentId, timeZone: "UTC" };
    await memory.save(writer, {
      page: "people/지훈 (동생)",
      text: "유학 계획",
      replaces: [],
      until: undefined,
    });
    await memory.forget(writer, { id: undefined, page: "people/지훈 (동생)" });

    const pending = await db
      .select({ id: agents.id, pending: agents.summaryRedactions })
      .from(agents);
    expect(pending.map((row) => row.pending)).toEqual([
      ["people/지훈 (동생)", "유학 계획"],
      ["people/지훈 (동생)", "유학 계획"],
    ]);
  });

  test("an agent with no summary has nothing to rewrite", async () => {
    const writer = { userId, agentId, timeZone: "UTC" };
    const entry = await memory.save(writer, {
      page: "me",
      text: "커피를 좋아한다",
      replaces: [],
      until: undefined,
    });
    await memory.forget(writer, { id: entry, page: undefined });
    const [row] = await db.select({ pending: agents.summaryRedactions }).from(agents);
    expect(row?.pending).toEqual([]);
  });

  test("a failed rewrite keeps the subject for a later try and the conversation goes on", async () => {
    await summarized();
    const writer = { userId, agentId, timeZone: "UTC" };
    const entry = await memory.save(writer, {
      page: "me",
      text: "유학 계획",
      replaces: [],
      until: undefined,
    });
    await memory.forget(writer, { id: entry, page: undefined });
    summarizerLog.reply = () => Promise.reject(new Error("summarizer down"));
    await say("안녕");

    const [row] = await db.select({ pending: agents.summaryRedactions }).from(agents);
    expect(row?.pending).toEqual(["유학 계획"]);
    const kept = await db.select().from(summaries).where(eq(summaries.agentId, agentId));
    expect(kept.map((summary) => summary.text)).toEqual([record]);
    expect(llm.prompts).toHaveLength(1);
  });
});

import { beforeEach, describe, expect, test } from "bun:test";
import { eq, sql } from "drizzle-orm";
import { db } from "../src/db";
import { devices } from "../src/db/schema/devices";
import { jobRuns, jobs } from "../src/db/schema/jobs";
import { settle, tick } from "../src/lib/jobs";
import { llm } from "./fakes";
import {
  bearer,
  client,
  codeFor,
  createAgent,
  listMessages,
  resetDatabase,
  send,
  signIn,
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

function inMinutes(minutes: number): string {
  return Temporal.Now.plainDateTimeISO("UTC")
    .add({ minutes })
    .round({ smallestUnit: "minute", roundingMode: "ceil" })
    .toString({ smallestUnit: "minute" });
}

async function signInDevice(email: string): Promise<string> {
  await client.auth["email-code"].$post({ json: { email } });
  const response = await client.auth["sign-in"].$post({
    json: {
      email,
      code: codeFor(email),
      device: { installationId: crypto.randomUUID(), name: "iPhone", platform: "ios" },
    },
  });
  if (!response.ok) throw new Error(`sign-in failed with ${response.status}`);
  return (await response.json()).token;
}

async function schedules() {
  const response = await client.agents[":id"].schedules.$get(
    { param: { id: agentId } },
    bearer(token),
  );
  if (!response.ok) throw new Error(`list schedules failed with ${response.status}`);
  return (await response.json()).schedules;
}

async function dueJob(minutesLate: number, rrule: string | null = null): Promise<string> {
  const [job] = await db
    .insert(jobs)
    .values({
      agentId,
      title: "오늘 일정 정리",
      instruction: "오늘 일정 정리해서 알려주기",
      start: "2026-01-01T08:00",
      rrule,
      nextRunAt: sql`clock_timestamp() - make_interval(mins => ${minutesLate})`,
    })
    .returning({ id: jobs.id });
  if (!job) throw new Error("job insert returned no row");
  return job.id;
}

async function runJobs(): Promise<void> {
  await tick();
  await settle();
}

function lastUserTexts() {
  return llm.prompts
    .at(-1)
    ?.findLast((message) => message.role === "user")
    ?.content.map((part) => (part.type === "text" ? part.text : ""));
}

describe("schedules", () => {
  test("the agent creates a schedule that the settings list shows", async () => {
    const start = inMinutes(60);
    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 1
          ? {
              toolName: "schedule_create",
              input: {
                title: "물 마시기",
                instruction: "물 마시라고 알려주기",
                start,
                rrule: "FREQ=DAILY",
              },
            }
          : "매일 알려드릴게요",
      );
    await send(token, agentId, "매일 이 시간에 물 마시라고 알려줘");
    await waitForIdle(agentId);

    const [schedule] = await schedules();
    expect(schedule).toMatchObject({ title: "물 마시기", repeats: true });
    expect(schedule?.nextRunAt).toBe(`${start}:00.000Z`);
    expect((await listMessages(token, agentId)).some((message) => message.scheduled)).toBe(true);
  });

  test("a due schedule reports as the agent's next message, and a one-time one is then gone", async () => {
    await dueJob(5);
    llm.reply = () => Promise.resolve(JSON.stringify({ message: "오늘은 일정이 없어요" }));
    await runJobs();
    await waitFor(async () => (await listMessages(token, agentId)).length > 0);
    await waitForIdle(agentId);

    const messages = await listMessages(token, agentId);
    expect(messages.map(({ role, text }) => ({ role, text }))).toEqual([
      { role: "assistant", text: "오늘은 일정이 없어요" },
    ]);
    expect(await schedules()).toEqual([]);
    expect(llm.prompts[0]?.at(-1)?.content).toContainEqual({
      type: "text",
      text: "Schedule: 오늘 일정 정리해서 알려주기",
    });
  });

  test("the agent knows which schedule its report came from", async () => {
    await dueJob(5, "FREQ=DAILY");
    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 1 ? JSON.stringify({ message: "오늘은 일정이 없어요" }) : "네",
      );
    await runJobs();
    await waitFor(async () => (await listMessages(token, agentId)).length > 0);
    await waitForIdle(agentId);

    await send(token, agentId, "고마워");
    await waitForIdle(agentId);
    const roles = llm.prompts.at(-1)?.map((message) => message.role);
    expect(roles).toEqual(["system", "user", "assistant", "user"]);
    expect(llm.prompts.at(-1)?.[1]?.content).toContainEqual({
      type: "text",
      text: "Scheduled run: 오늘 일정 정리",
    });
  });

  test("a quiet run leaves no message and a repeating schedule waits for its next run", async () => {
    await dueJob(5, "FREQ=DAILY");
    llm.reply = () => Promise.resolve(JSON.stringify({ message: null }));
    await runJobs();

    expect(await listMessages(token, agentId)).toEqual([]);
    const [schedule] = await schedules();
    expect(new Date(schedule?.nextRunAt ?? 0).getTime()).toBeGreaterThan(Date.now());
  });

  test("a run more than an hour late is recorded as missed instead of run", async () => {
    await dueJob(120, "FREQ=DAILY");
    await runJobs();

    expect(llm.prompts).toEqual([]);
    expect((await schedules())[0]?.problem).toBe("missed");
  });

  test("fires each due schedule once", async () => {
    const jobId = await dueJob(5, "FREQ=DAILY");
    llm.reply = () => Promise.resolve(JSON.stringify({ message: null }));
    await Promise.all([tick(), tick(), tick()]);
    await settle();

    const runs = await db.select().from(jobRuns).where(eq(jobRuns.jobId, jobId));
    expect(runs).toHaveLength(1);
  });

  test("tells the agent about schedule changes it did not make", async () => {
    const jobId = await dueJob(-60, "FREQ=DAILY");
    const pause = await client.agents[":id"].schedules[":scheduleId"].$patch(
      { param: { id: agentId, scheduleId: jobId }, json: { enabled: false } },
      bearer(token),
    );
    expect(pause.status).toBe(200);

    await send(token, agentId, "안녕");
    await waitForIdle(agentId);
    expect(lastUserTexts()?.find((text) => text.startsWith("Schedules:"))).toEndWith("(paused)");

    await send(token, agentId, "또 왔어");
    await waitForIdle(agentId);
    expect(lastUserTexts()).toEqual(["또 왔어"]);
  });

  test("schedules follow the user to a new time zone", async () => {
    const jobId = await dueJob(-60 * 24, "FREQ=DAILY");
    const before = await schedules();
    await client.me.$patch({ json: { timeZone: "Asia/Seoul" } }, bearer(token));
    const after = await schedules();

    expect(after[0]?.nextRunAt).not.toBe(before[0]?.nextRunAt);
    const [job] = await db.select().from(jobs).where(eq(jobs.id, jobId));
    const local = job?.nextRunAt?.toTemporalInstant().toZonedDateTimeISO("Asia/Seoul");
    expect(local?.toPlainTime().toString()).toBe("08:00:00");
  });

  test("a push token belongs to one signed-in device", async () => {
    const pushToken = "a".repeat(64);
    const first = await signInDevice("minho@example.com");
    const second = await signInDevice("other@example.com");
    await client.me["push-token"].$put({ json: { token: pushToken } }, bearer(first));
    const response = await client.me["push-token"].$put(
      { json: { token: pushToken } },
      bearer(second),
    );
    expect(response.status).toBe(204);

    const holders = await db
      .select({ userId: devices.userId })
      .from(devices)
      .where(eq(devices.pushToken, pushToken));
    expect(holders).toHaveLength(1);
  });

  test("a session not signed in as a device cannot take push tokens", async () => {
    const response = await client.me["push-token"].$put(
      { json: { token: "b".repeat(64) } },
      bearer(token),
    );
    expect(response.status).toBe(409);
  });
});

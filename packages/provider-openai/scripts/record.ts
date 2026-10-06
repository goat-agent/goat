import type { Event, Provider } from "@goat/provider";
import {
  checkStream,
  type Fetch,
  type Fixture,
  type FixtureName,
  recordFetch,
  scenarioInput,
  secondTurn,
} from "@goat/provider/testing";
import { z } from "zod";
import { apiKey, chatgpt } from "../src/index.ts";

interface Recording {
  readonly name: FixtureName;
  readonly fixtures: readonly Fixture[];
  readonly problem?: string;
}

type Account = "api" | "chatgpt";

const effort = "low";
const replies: readonly FixtureName[] = [
  "text",
  "reasoning",
  "tool_call",
  "parallel_tool_calls",
  "same_producer_replay",
  "foreign_replay",
];

const variable = z
  .string()
  .optional()
  .transform((value) => (value === "" ? undefined : value));

const environment = z.object({ OPENAI_API_KEY: variable, OPENAI_CHATGPT_TOKEN: variable });

const usage = z.union([
  z.tuple([z.literal("auth")]),
  z.tuple([z.enum(["api", "chatgpt"]), z.string().min(1)]),
]);

function provider(account: Account, credential: string, fetch: Fetch): Provider {
  return account === "api"
    ? apiKey({ key: credential, fetch })
    : chatgpt({ token: credential, fetch });
}

async function collect(stream: AsyncIterable<Event>): Promise<readonly Event[]> {
  const events: Event[] = [];
  for await (const event of stream) {
    events.push(event);
  }
  return events;
}

function problemOf(events: readonly Event[], expected: "reply" | "auth"): string | undefined {
  const checked = checkStream(events);
  if (!checked.ok) {
    return checked.error.join(" ");
  }
  const { stop } = checked.value;
  if (expected === "auth") {
    return stop.reason === "error" && stop.error.kind === "auth"
      ? undefined
      : `expected an auth error, got ${JSON.stringify(stop)}`;
  }
  return stop.reason === "error" ? `${stop.error.kind}: ${stop.error.message}` : undefined;
}

function recording(
  name: FixtureName,
  fixtures: readonly Fixture[],
  problem: string | undefined,
): Recording {
  return { name, fixtures, ...(problem === undefined ? {} : { problem }) };
}

async function recordReply(
  account: Account,
  credential: string,
  model: string,
  name: FixtureName,
): Promise<Recording> {
  const recorder = recordFetch(fetch);
  const subject = provider(account, credential, recorder.fetch).model(model);
  const input = scenarioInput(name, effort);
  const first = await collect(subject.stream(input));
  const end = first.at(-1);
  const problem = problemOf(first, "reply");
  if (problem !== undefined || name !== "same_producer_replay" || end?.type !== "end") {
    return recording(name, await recorder.fixtures(), problem);
  }
  const second = await collect(subject.stream(secondTurn(input, end.message)));
  return recording(name, await recorder.fixtures(), problemOf(second, "reply"));
}

async function recordAuthError(): Promise<Recording> {
  const recorder = recordFetch(fetch);
  const subject = apiKey({ key: "sk-invalid", fetch: recorder.fetch }).model("gpt-6-luna");
  const events = await collect(subject.stream(scenarioInput("auth_error")));
  return recording("auth_error", await recorder.fixtures(), problemOf(events, "auth"));
}

async function save(
  file: string,
  recordings: readonly Recording[],
  header: Readonly<Record<string, string>>,
): Promise<readonly string[]> {
  const problems = recordings.flatMap((item) =>
    item.problem === undefined ? [] : [`${item.name}: ${item.problem}`],
  );
  if (problems.length === 0) {
    const fixtures = Object.fromEntries(recordings.map((item) => [item.name, item.fixtures]));
    await Bun.write(
      new URL(`../fixtures/${file}`, import.meta.url),
      `${JSON.stringify({ ...header, fixtures }, null, 2)}\n`,
    );
  }
  return problems;
}

async function main(): Promise<readonly string[]> {
  const args = usage.safeParse(process.argv.slice(2));
  if (!args.success) {
    return ["Usage: bun run record auth | bun run record <api|chatgpt> <model>"];
  }
  if (args.data[0] === "auth") {
    return save("auth.json", [await recordAuthError()], {});
  }
  const [account, model] = args.data;
  const env = environment.safeParse(process.env);
  if (!env.success) {
    return [z.prettifyError(env.error)];
  }
  const credential = account === "api" ? env.data.OPENAI_API_KEY : env.data.OPENAI_CHATGPT_TOKEN;
  if (credential === undefined) {
    return [`Set ${account === "api" ? "OPENAI_API_KEY" : "OPENAI_CHATGPT_TOKEN"}.`];
  }
  const recordings = await Promise.all(
    replies.map(async (name) => recordReply(account, credential, model, name)),
  );
  return save(`${account}.json`, recordings, { model, effort });
}

const problems = await main();
if (problems.length > 0) {
  console.error([...problems, "Nothing was saved."].join("\n"));
  process.exitCode = 1;
}

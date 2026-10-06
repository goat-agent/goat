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
import { apiKey, oauth, type Options } from "../src/index.ts";

interface Recording {
  readonly name: FixtureName;
  readonly fixtures: readonly Fixture[];
  readonly problem?: string;
}

const effort = "high";
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

type Credential =
  | { readonly kind: "key"; readonly key: string; readonly options: Options }
  | { readonly kind: "oauth"; readonly token: string; readonly options: Options };

const environment = z.object({
  ANTHROPIC_BASE_URL: variable.pipe(z.url().optional()),
  ANTHROPIC_AUTH_TOKEN: variable,
  ANTHROPIC_API_KEY: variable,
  CLAUDE_CODE_OAUTH_TOKEN: variable,
});

const usage = z.tuple([z.string().min(1)]);

function credentialOf(env: Readonly<z.output<typeof environment>>): Credential | undefined {
  const options = env.ANTHROPIC_BASE_URL === undefined ? {} : { baseUrl: env.ANTHROPIC_BASE_URL };
  const key = env.ANTHROPIC_AUTH_TOKEN ?? env.ANTHROPIC_API_KEY;
  if (key !== undefined) {
    return { kind: "key", key, options };
  }
  const token = env.CLAUDE_CODE_OAUTH_TOKEN;
  return token === undefined ? undefined : { kind: "oauth", token, options };
}

function provider(source: Credential, fetch: Fetch): Provider {
  const options: Options = { ...source.options, fetch };
  return source.kind === "key"
    ? apiKey({ key: source.key, ...options })
    : oauth({ token: source.token, ...options });
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
  source: Credential,
  model: string,
  name: FixtureName,
): Promise<Recording> {
  const recorder = recordFetch(fetch);
  const subject = provider(source, recorder.fetch).model(model);
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

async function recordAuthError(model: string): Promise<Recording> {
  const recorder = recordFetch(fetch);
  const subject = apiKey({ key: "sk-ant-invalid", fetch: recorder.fetch }).model(model);
  const events = await collect(subject.stream(scenarioInput("auth_error")));
  return recording("auth_error", await recorder.fixtures(), problemOf(events, "auth"));
}

async function record(source: Credential, model: string): Promise<readonly string[]> {
  const recordings = await Promise.all([
    ...replies.map(async (name) => recordReply(source, model, name)),
    recordAuthError(model),
  ]);
  const problems = recordings.flatMap((item) =>
    item.problem === undefined ? [] : [`${item.name}: ${item.problem}`],
  );
  if (problems.length === 0) {
    const fixtures = Object.fromEntries(recordings.map((item) => [item.name, item.fixtures]));
    await Bun.write(
      new URL("../fixtures/recorded.json", import.meta.url),
      `${JSON.stringify({ model, effort, fixtures }, null, 2)}\n`,
    );
  }
  return problems;
}

async function main(): Promise<readonly string[]> {
  const env = environment.safeParse(process.env);
  if (!env.success) {
    return [z.prettifyError(env.error)];
  }
  const source = credentialOf(env.data);
  if (source === undefined) {
    return ["Set ANTHROPIC_AUTH_TOKEN, ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN."];
  }
  const args = usage.safeParse(process.argv.slice(2));
  return args.success ? record(source, args.data[0]) : ["Usage: bun run record <model>"];
}

const problems = await main();
if (problems.length > 0) {
  console.error([...problems, "Nothing was saved."].join("\n"));
  process.exitCode = 1;
}

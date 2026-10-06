import { describe, expect, test } from "bun:test";
import type { ErrorKind } from "../error.ts";
import type { Event } from "../event.ts";
import type { AgentMessage } from "../message.ts";
import type { Input, Provider } from "../model.ts";
import { checkStream } from "./check-stream.ts";
import {
  type Chunking,
  type Fetch,
  type Fixture,
  type RecordedRequest,
  recordedFetch,
} from "./fetch.ts";
import {
  type FixtureName,
  foreignMarkers,
  scenarioInput,
  secondTurn,
  unsupportedEffort,
} from "./scenarios.ts";
import {
  expectMalformedCall,
  expectParallelCalls,
  expectReasoningFirst,
  expectText,
  expectWeatherCall,
  opaqueStrings,
} from "./verify.ts";

export interface ConformanceSubject {
  readonly name: string;
  readonly model: string;
  readonly effort?: string;
  readonly offlineModelInfo: boolean;
  readonly create: (fetch: Fetch) => Provider;
  readonly fixtures: Readonly<Partial<Record<FixtureName, readonly Fixture[]>>>;
}

interface Turn {
  readonly events: readonly Event[];
  readonly message: AgentMessage;
  readonly requests: readonly RecordedRequest[];
}

interface Check {
  readonly title: string;
  readonly fixture?: FixtureName;
  readonly run: (subject: ConformanceSubject, fixtures: readonly Fixture[]) => Promise<void>;
}

const replies: readonly {
  readonly name: FixtureName;
  readonly verify: (message: AgentMessage) => void;
}[] = [
  { name: "text", verify: expectText },
  { name: "reasoning", verify: expectReasoningFirst },
  { name: "tool_call", verify: expectWeatherCall },
  { name: "parallel_tool_calls", verify: expectParallelCalls },
  { name: "malformed_tool_input", verify: expectMalformedCall },
];

const failures: readonly { readonly name: FixtureName; readonly kind: ErrorKind }[] = [
  { name: "auth_error", kind: "auth" },
  { name: "rate_limit", kind: "rate_limit" },
  { name: "server_error", kind: "unavailable" },
  { name: "overflow", kind: "overflow" },
];

const checks: readonly Check[] = [
  ...replies.map(({ name, verify }): Check => ({
    title: name,
    fixture: name,
    run: async (subject, fixtures) => {
      verify((await turn(subject, fixtures, scenarioInput(name, subject.effort))).message);
    },
  })),
  ...replies.map(({ name }): Check => ({
    title: `${name} is independent of chunk boundaries`,
    fixture: name,
    run: async (subject, fixtures) => expectSameEventsForEveryChunking(subject, fixtures, name),
  })),
  ...failures.map(({ name, kind }): Check => ({
    title: name,
    fixture: name,
    run: async (subject, fixtures) => expectFailure(subject, fixtures, name, kind),
  })),
  { title: "midstream_error", fixture: "midstream_error", run: expectMidstreamError },
  { title: "same_producer_replay", fixture: "same_producer_replay", run: expectSameProducerReplay },
  { title: "foreign_replay", fixture: "foreign_replay", run: expectForeignReplay },
  { title: "abort", fixture: "text", run: expectAbort },
  { title: "break cancels the request", fixture: "text", run: expectBreakCancels },
];

export function conformance(subject: ConformanceSubject): void {
  describe(`${subject.name} conformance`, () => {
    for (const check of checks) {
      const fixtures = check.fixture === undefined ? [] : subject.fixtures[check.fixture];
      test.skipIf(fixtures === undefined)(check.title, async () =>
        check.run(subject, fixtures ?? []),
      );
    }
    test.skipIf(!subject.offlineModelInfo)("unsupported effort fails before sending", async () =>
      expectUnsupportedEffort(subject),
    );
  });
}

async function turn(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
  input: Input,
  chunking: Chunking = "recorded",
): Promise<Turn> {
  const transport = recordedFetch(fixtures, chunking);
  const events = await collect(subject.create(transport.fetch).model(subject.model).stream(input));
  return { events, message: checked(events), requests: transport.requests };
}

async function collect(stream: AsyncIterable<Event>): Promise<readonly Event[]> {
  const events: Event[] = [];
  for await (const event of stream) {
    events.push(event);
  }
  return events;
}

function checked(events: readonly Event[]): AgentMessage {
  const result = checkStream(events);
  expect(result.ok ? [] : result.error).toEqual([]);
  if (!result.ok) {
    throw new Error(result.error.join("\n"));
  }
  return result.value;
}

function errorOf(
  message: AgentMessage,
): { readonly kind: ErrorKind; readonly retryAfter?: number } | undefined {
  return message.stop.reason === "error" ? message.stop.error : undefined;
}

async function expectSameEventsForEveryChunking(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
  name: FixtureName,
): Promise<void> {
  const input = scenarioInput(name, subject.effort);
  const recorded = await turn(subject, fixtures, input, "recorded");
  const bytes = await turn(subject, fixtures, input, "bytes");
  const whole = await turn(subject, fixtures, input, "whole");
  expect(bytes.events).toEqual(recorded.events);
  expect(whole.events).toEqual(recorded.events);
}

async function expectFailure(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
  name: FixtureName,
  kind: ErrorKind,
): Promise<void> {
  const error = errorOf(
    (await turn(subject, fixtures, scenarioInput(name, subject.effort))).message,
  );
  expect(error?.kind).toBe(kind);
  if (kind === "rate_limit") {
    expect(error?.retryAfter ?? 0).toBeGreaterThan(0);
  }
}

async function expectMidstreamError(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
): Promise<void> {
  const { message } = await turn(
    subject,
    fixtures,
    scenarioInput("midstream_error", subject.effort),
  );
  expect(message.stop.reason).toBe("error");
  expect(message.parts.length).toBeGreaterThan(0);
}

async function expectSameProducerReplay(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
): Promise<void> {
  const transport = recordedFetch(fixtures);
  const model = subject.create(transport.fetch).model(subject.model);
  const firstInput = scenarioInput("same_producer_replay", subject.effort);
  const first = checked(await collect(model.stream(firstInput)));
  checked(await collect(model.stream(secondTurn(firstInput, first))));
  const body = transport.requests[1]?.body ?? "";
  for (const value of opaqueStrings(first)) {
    expect(body).toContain(JSON.stringify(value).slice(1, -1));
  }
}

async function expectForeignReplay(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
): Promise<void> {
  const { requests } = await turn(
    subject,
    fixtures,
    scenarioInput("foreign_replay", subject.effort),
  );
  const body = requests[0]?.body ?? "";
  expect(body).toContain(foreignMarkers.text);
  expect(body).not.toContain(foreignMarkers.signature);
  expect(body).not.toContain(foreignMarkers.reasoning);
  expect(body).not.toContain(foreignMarkers.textId);
}

async function expectAbort(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
): Promise<void> {
  const transport = recordedFetch(fixtures, "bytes");
  const controller = new AbortController();
  const model = subject.create(transport.fetch).model(subject.model);
  const events: Event[] = [];
  for await (const event of model.stream(scenarioInput("text"), { signal: controller.signal })) {
    events.push(event);
    if (event.type === "part_delta") {
      controller.abort();
    }
  }
  expect(checked(events).stop.reason).toBe("aborted");
  expect(transport.requests[0]?.signal?.aborted).toBe(true);
}

async function expectBreakCancels(
  subject: ConformanceSubject,
  fixtures: readonly Fixture[],
): Promise<void> {
  const transport = recordedFetch(fixtures, "bytes");
  for await (const event of subject
    .create(transport.fetch)
    .model(subject.model)
    .stream(scenarioInput("text"))) {
    if (event.type === "part_delta") {
      break;
    }
  }
  await new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
  expect(transport.requests[0]?.signal?.aborted).toBe(true);
}

async function expectUnsupportedEffort(subject: ConformanceSubject): Promise<void> {
  const transport = recordedFetch([]);
  const input = { ...scenarioInput("text"), effort: unsupportedEffort };
  const events = await collect(subject.create(transport.fetch).model(subject.model).stream(input));
  expect(errorOf(checked(events))?.kind).toBe("unsupported");
  expect(transport.requests).toHaveLength(0);
}

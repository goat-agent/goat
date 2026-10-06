import { expect, test } from "bun:test";
import type { AgentMessage, Event } from "../provider/index.ts";
import { checkStream } from "../provider/testing/index.ts";
import { catalogProfile } from "./catalog.ts";
import {
  call,
  completed,
  created,
  failed,
  incomplete,
  message,
  reasoning,
  type WireEvent,
} from "./fixtures.ts";
import { Translation } from "./translation.ts";

const producer = { provider: "openai", model: "gpt-6-luna" };

function translate(events: readonly WireEvent[], model = "gpt-6-luna"): readonly Event[] {
  const translation = new Translation(producer, catalogProfile(model, true));
  return events.flatMap((event) => translation.push(JSON.parse(JSON.stringify(event))));
}

function final(events: readonly Event[]): AgentMessage {
  const result = checkStream(events);
  if (!result.ok) {
    throw new Error(result.error.join("\n"));
  }
  return result.value;
}

test("keeps one reasoning part per summary and the encrypted item on the last", () => {
  const events = translate([
    created(),
    ...reasoning(0, "rs_1", ["**Plan**", "Then answer."], "enc-final"),
    ...message(1, "msg_1", ["Done."]),
    completed(),
  ]);
  expect(final(events).parts).toEqual([
    { type: "reasoning", text: "**Plan**", opaque: { id: "rs_1" } },
    {
      type: "reasoning",
      text: "Then answer.",
      opaque: { id: "rs_1", encrypted_content: "enc-final" },
    },
    { type: "text", text: "Done.", opaque: { id: "msg_1", phase: "final_answer" } },
  ]);
});

test("keeps reasoning without a summary as an empty part", () => {
  const events = translate([created(), ...reasoning(0, "rs_1", [], "enc"), completed()]);
  expect(final(events).parts).toEqual([
    { type: "reasoning", text: "", opaque: { id: "rs_1", encrypted_content: "enc" } },
  ]);
});

test("streams tool arguments and keeps the item id for replay", () => {
  const events = translate([
    created(),
    ...call(0, "fc_1", "call_1", "get_weather", '{"city":', '"Seoul"}'),
    completed(),
  ]);
  expect(events.filter((event) => event.type === "part_delta")).toEqual([
    { type: "part_delta", index: 0, text: '{"city":' },
    { type: "part_delta", index: 0, text: '"Seoul"}' },
  ]);
  expect(final(events).parts).toEqual([
    {
      type: "tool_call",
      id: "call_1",
      name: "get_weather",
      input: '{"city":"Seoul"}',
      opaque: { id: "fc_1" },
    },
  ]);
});

test("ends with a refusal when the model refuses", () => {
  const refusal: readonly WireEvent[] = [
    created(),
    {
      type: "response.output_item.done",
      output_index: 0,
      item: {
        id: "msg_1",
        type: "message",
        role: "assistant",
        status: "completed",
        content: [{ type: "refusal", refusal: "I can't help with that." }],
      },
    },
    completed(),
  ];
  const result = final(translate(refusal));
  expect(result.stop).toEqual({ reason: "refusal" });
  expect(result.parts[0]).toMatchObject({ type: "text", text: "I can't help with that." });
  expect(final(translate([created(), incomplete("content_filter")])).stop).toEqual({
    reason: "refusal",
  });
});

test("maps early stops to length or an error", () => {
  expect(final(translate([created(), incomplete("max_output_tokens")])).stop).toEqual({
    reason: "length",
  });
  expect(final(translate([created(), incomplete("max_messages")])).stop).toMatchObject({
    reason: "error",
    error: { kind: "unknown" },
  });
  expect(
    final(translate([created(), failed("context_length_exceeded", "Too long.")])).stop,
  ).toEqual({ reason: "error", error: { kind: "overflow", message: "Too long." } });
});

test("reads error events in both the documented and the nested shape", () => {
  const flat = {
    type: "error",
    code: "rate_limit_exceeded",
    message: "Try again in 2s.",
    param: null,
  };
  const nested = { type: "error", error: { code: "server_is_overloaded", message: "Busy." } };
  expect(final(translate([created(), flat])).stop).toEqual({
    reason: "error",
    error: { kind: "rate_limit", message: "Try again in 2s.", retryAfter: 2000 },
  });
  expect(final(translate([created(), nested])).stop).toEqual({
    reason: "error",
    error: { kind: "unavailable", message: "Busy." },
  });
});

test("keeps unknown items verbatim", () => {
  const compaction = { id: "cmp_1", type: "compaction", encrypted_content: "enc" };
  const events = translate([
    created(),
    { type: "response.output_item.added", output_index: 0, item: compaction },
    { type: "response.output_item.done", output_index: 0, item: compaction },
    completed(),
  ]);
  expect(final(events).parts).toEqual([{ type: "opaque", data: compaction }]);
});

test("ignores events it does not use", () => {
  const events = translate([
    created(),
    { type: "response.in_progress", response: {} },
    { type: "codex.rate_limits", primary: {} },
    ...message(0, "msg_1", ["Hi."]),
    completed(),
  ]);
  expect(final(events).stop).toEqual({ reason: "done" });
});

test("counts cached input inside the input total and prices it", () => {
  const events = translate(
    [
      created(),
      ...message(0, "msg_1", ["Hi."]),
      completed({ input: 1000, cached: 600, written: 100, output: 50, reasoning: 10 }),
    ],
    "gpt-6.1-sol",
  );
  expect(final(events).usage).toEqual({
    input: { total: 1000, cacheRead: 600, cacheWrite: 100 },
    output: { total: 50, reasoning: 10 },
    cost: (300 * 2 + 600 * 0.1 + 100 * 2.5 + 50 * 10) / 1_000_000,
  });
});

test("applies the service tier and long-context prices", () => {
  const flex = final(
    translate([created(), completed({ input: 1000, output: 100, tier: "flex" })], "gpt-6-luna"),
  );
  expect(flex.usage?.cost).toBeCloseTo(((1000 * 0.1 + 100 * 0.5) * 0.5) / 1_000_000);
  const long = final(translate([created(), completed({ input: 300_000, output: 100 })]));
  expect(long.usage?.cost).toBeCloseTo((300_000 * 0.1 * 2 + 100 * 0.5 * 1.5) / 1_000_000);
  const unknown = final(translate([created(), completed({ tier: "scale" })]));
  expect(unknown.usage?.cost).toBeUndefined();
});

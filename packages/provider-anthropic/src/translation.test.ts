import { expect, test } from "bun:test";
import type { AgentMessage, Event } from "../provider/index.ts";
import { checkStream } from "../provider/testing/index.ts";
import { Decoder } from "../sse/index.ts";
import { catalogProfile } from "./catalog.ts";
import { block, finish, sse, start, text, thinking, toolUse, type WireEvent } from "./fixtures.ts";
import { Translation } from "./translation.ts";

const producer = { provider: "anthropic", model: "claude-opus-5-5" };

function translate(events: readonly WireEvent[]): readonly Event[] {
  const translation = new Translation(producer, catalogProfile("claude-opus-5-5")?.price);
  const decoded = new Decoder().push(sse(events));
  const emitted = decoded.flatMap((event) => translation.push(event));
  return translation.finished ? emitted : [...emitted, ...translation.close({ reason: "aborted" })];
}

function messageOf(events: readonly WireEvent[]): AgentMessage {
  const result = checkStream(translate(events));
  if (!result.ok) {
    throw new Error(result.error.join("\n"));
  }
  return result.value;
}

test("streams text and reports usage with its cost", () => {
  const events = translate([
    start(1000),
    ...text(0, "Hello", " there."),
    ...finish("end_turn", 500),
  ]);
  expect(events.filter((event) => event.type === "part_delta")).toEqual([
    { type: "part_delta", index: 0, text: "Hello" },
    { type: "part_delta", index: 0, text: " there." },
  ]);
  const message = messageOf([
    start(1000),
    ...text(0, "Hello", " there."),
    ...finish("end_turn", 500),
  ]);
  expect(message.parts).toEqual([{ type: "text", text: "Hello there." }]);
  expect(message.usage).toEqual({ input: { total: 1000 }, output: { total: 500 }, cost: 0.014 });
  expect(message.stop).toEqual({ reason: "done" });
});

test("keeps the thinking signature as opaque data and out of the deltas", () => {
  const events = [
    start(),
    ...thinking(0, "Plan.", "sig-1"),
    ...text(1, "Done."),
    ...finish("end_turn"),
  ];
  expect(translate(events).filter((event) => event.type === "part_delta")).toEqual([
    { type: "part_delta", index: 0, text: "Plan." },
    { type: "part_delta", index: 1, text: "Done." },
  ]);
  expect(messageOf(events).parts).toEqual([
    { type: "reasoning", text: "Plan.", opaque: { signature: "sig-1" } },
    { type: "text", text: "Done." },
  ]);
});

test("streams tool input and closes an empty input as an object", () => {
  const message = messageOf([
    start(),
    ...toolUse(0, "toolu_1", "get_weather", '{"city": ', '"Seoul"}'),
    ...toolUse(1, "toolu_2", "list_files"),
    ...finish("tool_use"),
  ]);
  expect(message.parts).toEqual([
    { type: "tool_call", id: "toolu_1", name: "get_weather", input: '{"city": "Seoul"}' },
    { type: "tool_call", id: "toolu_2", name: "list_files", input: "{}" },
  ]);
});

test("keeps citations and unknown fields for replay", () => {
  const citation = { type: "char_location", cited_text: "sky", document_index: 0 };
  const message = messageOf([
    start(),
    ...block(0, { type: "text", text: "", citations: null }, [
      { type: "text_delta", text: "Blue." },
      { type: "citations_delta", citation },
    ]),
    ...block(
      1,
      { type: "tool_use", id: "toolu_1", name: "run", input: {}, caller: { type: "direct" } },
      [],
    ),
    ...finish("end_turn"),
  ]);
  expect(message.parts).toEqual([
    { type: "text", text: "Blue.", opaque: { citations: [citation] } },
    {
      type: "tool_call",
      id: "toolu_1",
      name: "run",
      input: "{}",
      opaque: { caller: { type: "direct" } },
    },
  ]);
});

test("keeps blocks it does not understand verbatim", () => {
  const redacted = { type: "redacted_thinking", data: "encrypted" };
  const search = {
    type: "server_tool_use",
    id: "srvtoolu_1",
    name: "web_search",
    input: { query: "x" },
  };
  const message = messageOf([
    start(),
    ...block(0, redacted, []),
    ...block(1, search, []),
    ...finish("end_turn"),
  ]);
  expect(message.parts).toEqual([
    { type: "opaque", data: redacted },
    { type: "opaque", data: search },
  ]);
});

test("ignores pings, unknown events and unknown deltas", () => {
  const message = messageOf([
    start(),
    ["ping", { type: "ping" }],
    ...block(0, { type: "text", text: "" }, [
      { type: "text_delta", text: "Hi." },
      { type: "future_delta", value: 1 },
    ]),
    ["future_event", { type: "future_event" }],
    ...finish("end_turn"),
  ]);
  expect(message.parts).toEqual([{ type: "text", text: "Hi." }]);
});

function stopFor(reason: string): AgentMessage["stop"] {
  return messageOf([start(), ...text(0, "x"), ...finish(reason)]).stop;
}

test("maps stop reasons", () => {
  expect(stopFor("max_tokens")).toEqual({ reason: "length" });
  expect(stopFor("model_context_window_exceeded")).toEqual({ reason: "overflow" });
  expect(stopFor("refusal")).toEqual({ reason: "refusal" });
  expect(stopFor("stop_sequence")).toEqual({ reason: "done" });
  expect(stopFor("pause_turn")).toEqual({ reason: "done" });
});

test("ends an error event with the parts it already started", () => {
  const message = messageOf([
    start(),
    ...text(0, "Hel").slice(0, 2),
    ["error", { type: "error", error: { type: "overloaded_error", message: "Overloaded" } }],
  ]);
  expect(message.parts).toEqual([{ type: "text", text: "Hel" }]);
  expect(message.stop).toEqual({
    reason: "error",
    error: { kind: "unavailable", message: "Overloaded" },
  });
});

test("fails a message without a stop reason or with a broken event", () => {
  const missing = messageOf([start(), ["message_stop", { type: "message_stop" }]]);
  expect(missing.stop.reason === "error" && missing.stop.error.kind).toBe("malformed");
  const broken = messageOf([start(), ["content_block_start", { type: "content_block_start" }]]);
  expect(broken.stop.reason === "error" && broken.stop.error.kind).toBe("malformed");
});

test("closes open parts when the stream stops early", () => {
  const events = translate([start(), ...toolUse(0, "toolu_1", "run", '{"a"').slice(0, 2)]);
  expect(events.at(-2)).toEqual({
    type: "part_end",
    index: 0,
    part: { type: "tool_call", id: "toolu_1", name: "run", input: '{"a"' },
  });
  expect(events.at(-1)?.type).toBe("end");
});

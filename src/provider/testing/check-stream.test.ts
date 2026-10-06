import { expect, test } from "bun:test";
import type { Event } from "../event.ts";
import type { AgentMessage } from "../message.ts";
import type { ModelInfo } from "../model.ts";
import { checkStream } from "./check-stream.ts";
import { scriptedModel } from "./scripted-model.ts";

const info: ModelInfo = {
  id: "scripted",
  name: "Scripted",
  contextWindow: 1000,
  maxOutputTokens: 100,
  supports: { media: [], tools: true, outputSchema: false },
};

const reply: AgentMessage = {
  role: "agent",
  producer: { provider: "script", model: "scripted" },
  parts: [
    { type: "reasoning", text: "plan", opaque: { signature: "s1" } },
    { type: "text", text: "hello" },
    { type: "tool_call", id: "c1", name: "read_file", input: '{"path":"a.ts"}' },
    { type: "opaque", data: { block: 1 } },
  ],
  stop: { reason: "done" },
};

async function collect(stream: AsyncIterable<Event>): Promise<readonly Event[]> {
  const events: Event[] = [];
  for await (const event of stream) {
    events.push(event);
  }
  return events;
}

test("accepts a stream that keeps the contract", async () => {
  const model = scriptedModel({ provider: "script", info, replies: [reply] });
  const events = await collect(model.stream({ messages: [] }));
  expect(checkStream(events)).toEqual({ ok: true, value: reply });
});

test("ends with aborted when the signal fires", async () => {
  const model = scriptedModel({ provider: "script", info, replies: [reply] });
  const controller = new AbortController();
  const events: Event[] = [];
  for await (const event of model.stream({ messages: [] }, { signal: controller.signal })) {
    events.push(event);
    controller.abort();
  }
  const result = checkStream(events);
  expect(result.ok ? result.value.stop.reason : result.error).toBe("aborted");
});

test("reports a missing end", () => {
  expect(checkStream([])).toEqual({ ok: false, error: ["The stream has no end event."] });
});

test("reports every broken rule", () => {
  const text = { type: "text", text: "hi" } as const;
  const events: readonly Event[] = [
    { type: "part_delta", index: 0, text: "x" },
    { type: "part_start", index: 1, part: text },
    { type: "part_start", index: 0, part: text },
    { type: "part_end", index: 0, part: { type: "reasoning", text: "hi" } },
    { type: "end", message: { ...reply, parts: [] } },
    { type: "part_delta", index: 0, text: "late" },
  ];
  expect(checkStream(events)).toEqual({
    ok: false,
    error: [
      "part_delta for part 0 arrived before its part_start.",
      "part_start used index 1, expected 0.",
      "part 0 started as text and ended as reasoning.",
      "part 0 was never ended.",
      "end has 0 parts but 1 were started.",
      "A part_delta event arrived after end.",
    ],
  });
});

test("reports an end part that differs from its part_end", () => {
  const events: readonly Event[] = [
    { type: "part_start", index: 0, part: { type: "text", text: "" } },
    { type: "part_end", index: 0, part: { type: "text", text: "hi" } },
    { type: "end", message: { ...reply, parts: [{ type: "text", text: "bye" }] } },
  ];
  expect(checkStream(events)).toEqual({
    ok: false,
    error: ["end part 0 differs from its part_end."],
  });
});

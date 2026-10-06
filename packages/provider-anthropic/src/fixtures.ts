import type { Json } from "../provider/index.ts";
import type { Fixture, FixtureName } from "../provider/testing/index.ts";
import { encode } from "../sse/index.ts";

export type WireEvent = readonly [type: string, data: Json];

export function sse(events: readonly WireEvent[]): string {
  return events.map(([type, data]) => encode({ type, data: JSON.stringify(data) })).join("");
}

export function streamFixture(events: readonly WireEvent[]): Fixture {
  return { status: 200, headers: { "content-type": "text/event-stream" }, chunks: [sse(events)] };
}

export function errorFixture(
  status: number,
  type: string,
  message: string,
  headers: Readonly<Record<string, string>> = {},
): Fixture {
  return {
    status,
    headers: { "content-type": "application/json", ...headers },
    chunks: [JSON.stringify({ type: "error", error: { type, message }, request_id: "req_1" })],
  };
}

export function start(input = 12): WireEvent {
  return [
    "message_start",
    {
      type: "message_start",
      message: {
        id: "msg_1",
        type: "message",
        role: "assistant",
        model: "claude-opus-5-5",
        content: [],
        stop_reason: null,
        usage: { input_tokens: input, output_tokens: 1 },
      },
    },
  ];
}

export function text(index: number, ...pieces: readonly string[]): readonly WireEvent[] {
  return block(
    index,
    { type: "text", text: "" },
    pieces.map((piece) => ({ type: "text_delta", text: piece })),
  );
}

export function thinking(index: number, thought: string, signature: string): readonly WireEvent[] {
  return block(index, { type: "thinking", thinking: "", signature: "" }, [
    { type: "thinking_delta", thinking: thought },
    { type: "signature_delta", signature },
  ]);
}

export function toolUse(
  index: number,
  id: string,
  name: string,
  ...pieces: readonly string[]
): readonly WireEvent[] {
  return block(
    index,
    { type: "tool_use", id, name, input: {} },
    pieces.map((piece) => ({ type: "input_json_delta", partial_json: piece })),
  );
}

export function block(index: number, content: Json, deltas: readonly Json[]): readonly WireEvent[] {
  return [
    ["content_block_start", { type: "content_block_start", index, content_block: content }],
    ...deltas.map((delta): WireEvent => [
      "content_block_delta",
      { type: "content_block_delta", index, delta },
    ]),
    ["content_block_stop", { type: "content_block_stop", index }],
  ];
}

export function finish(stopReason: string, output = 20): readonly WireEvent[] {
  return [
    [
      "message_delta",
      {
        type: "message_delta",
        delta: { stop_reason: stopReason, stop_sequence: null },
        usage: { output_tokens: output },
      },
    ],
    ["message_stop", { type: "message_stop" }],
  ];
}

export const handwritten: Readonly<Partial<Record<FixtureName, readonly Fixture[]>>> = {
  malformed_tool_input: [
    streamFixture([
      start(),
      ...toolUse(0, "toolu_1", "get_weather", '{"city": "Seo'),
      ...finish("max_tokens"),
    ]),
  ],
  rate_limit: [
    errorFixture(429, "rate_limit_error", "Number of requests has exceeded your rate limit.", {
      "retry-after": "12",
    }),
  ],
  server_error: [errorFixture(529, "overloaded_error", "Overloaded")],
  overflow: [
    errorFixture(
      400,
      "invalid_request_error",
      "prompt is too long: 1000512 tokens > 1000000 maximum",
    ),
  ],
  midstream_error: [
    streamFixture([
      start(),
      ...text(0, "Hel").slice(0, 2),
      ["error", { type: "error", error: { type: "overloaded_error", message: "Overloaded" } }],
    ]),
  ],
};

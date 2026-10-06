import type { Json } from "@goat/provider";
import type { Fixture, FixtureName } from "@goat/provider/testing";
import { encode } from "@goat/sse";
import { asObject, type JsonObject } from "./json.ts";

export type WireEvent = { readonly type: string } & { readonly [key: string]: Json };

interface Counts {
  readonly input?: number;
  readonly cached?: number;
  readonly written?: number;
  readonly output?: number;
  readonly reasoning?: number;
  readonly tier?: string;
}

function sse(events: readonly WireEvent[]): string {
  return events
    .map((event, index) =>
      encode({ type: event.type, data: JSON.stringify({ ...event, sequence_number: index }) }),
    )
    .join("");
}

export function streamFixture(events: readonly WireEvent[]): Fixture {
  return { status: 200, headers: { "content-type": "text/event-stream" }, chunks: [sse(events)] };
}

export function errorFixture(
  status: number,
  code: string | null,
  text: string,
  headers: Readonly<Record<string, string>> = {},
  param: string | null = null,
): Fixture {
  return {
    status,
    headers: { "content-type": "application/json", ...headers },
    chunks: [
      JSON.stringify({ error: { message: text, type: "invalid_request_error", param, code } }),
    ],
  };
}

export function created(): WireEvent {
  return {
    type: "response.created",
    response: { id: "resp_1", object: "response", status: "in_progress", output: [] },
  };
}

export function reasoning(
  output: number,
  id: string,
  summary: readonly string[],
  encrypted: string,
): readonly WireEvent[] {
  const parts = summary.flatMap((text, index): readonly WireEvent[] => {
    const position = { item_id: id, output_index: output, summary_index: index };
    return [
      { type: "response.reasoning_summary_part.added", ...position, part: summaryText("") },
      { type: "response.reasoning_summary_text.delta", ...position, delta: text },
      { type: "response.reasoning_summary_text.done", ...position, text },
      { type: "response.reasoning_summary_part.done", ...position, part: summaryText(text) },
    ];
  });
  const item = { id, type: "reasoning", content: [] };
  return [
    {
      type: "response.output_item.added",
      output_index: output,
      item: { ...item, summary: [], encrypted_content: "gAAAA-incomplete" },
    },
    ...parts,
    {
      type: "response.output_item.done",
      output_index: output,
      item: {
        ...item,
        summary: summary.map((text) => summaryText(text)),
        encrypted_content: encrypted,
      },
    },
  ];
}

export function message(
  output: number,
  id: string,
  pieces: readonly string[],
  phase = "final_answer",
): readonly WireEvent[] {
  const position = { item_id: id, output_index: output, content_index: 0 };
  const text = pieces.join("");
  const item = { id, type: "message", role: "assistant", phase };
  return [
    {
      type: "response.output_item.added",
      output_index: output,
      item: { ...item, status: "in_progress", content: [] },
    },
    { type: "response.content_part.added", ...position, part: outputText("") },
    ...pieces.map((delta): WireEvent => ({
      type: "response.output_text.delta",
      ...position,
      delta,
    })),
    { type: "response.output_text.done", ...position, text },
    { type: "response.content_part.done", ...position, part: outputText(text) },
    {
      type: "response.output_item.done",
      output_index: output,
      item: { ...item, status: "completed", content: [outputText(text)] },
    },
  ];
}

export function call(
  output: number,
  id: string,
  callId: string,
  name: string,
  ...pieces: readonly string[]
): readonly WireEvent[] {
  const item = { id, type: "function_call", call_id: callId, name };
  const position = { item_id: id, output_index: output };
  return [
    {
      type: "response.output_item.added",
      output_index: output,
      item: { ...item, status: "in_progress", arguments: "" },
    },
    ...pieces.map((delta): WireEvent => ({
      type: "response.function_call_arguments.delta",
      ...position,
      delta,
    })),
    {
      type: "response.output_item.done",
      output_index: output,
      item: { ...item, status: "completed", arguments: pieces.join("") },
    },
  ];
}

export function completed(counts: Counts = {}): WireEvent {
  return terminal("response.completed", "completed", counts);
}

export function incomplete(reason: string, counts: Counts = {}): WireEvent {
  const event = terminal("response.incomplete", "incomplete", counts);
  return { ...event, response: { ...response(event), incomplete_details: { reason } } };
}

export function failed(code: string, text: string): WireEvent {
  const event = terminal("response.failed", "failed", {});
  return { ...event, response: { ...response(event), error: { code, message: text } } };
}

function terminal(type: string, status: string, counts: Counts): WireEvent {
  return {
    type,
    response: {
      id: "resp_1",
      object: "response",
      status,
      output: [],
      service_tier: counts.tier ?? "default",
      usage: {
        input_tokens: counts.input ?? 12,
        input_tokens_details: {
          cached_tokens: counts.cached ?? 0,
          cache_write_tokens: counts.written ?? 0,
        },
        output_tokens: counts.output ?? 20,
        output_tokens_details: { reasoning_tokens: counts.reasoning ?? 0 },
      },
    },
  };
}

function response(event: WireEvent): JsonObject {
  return asObject(event["response"]) ?? {};
}

function summaryText(text: string): Json {
  return { type: "summary_text", text };
}

function outputText(text: string): Json {
  return { type: "output_text", text, annotations: [], logprobs: [] };
}

const weather = (output: number, suffix: string, city: string): readonly WireEvent[] =>
  call(output, `fc_${suffix}`, `call_${suffix}`, "get_weather", '{"city":', `"${city}"}`);

export const handwritten: Readonly<Partial<Record<FixtureName, readonly Fixture[]>>> = {
  text: [streamFixture([created(), ...message(0, "msg_1", ["Hel", "lo!"]), completed()])],
  reasoning: [
    streamFixture([
      created(),
      ...reasoning(0, "rs_1", ["**Multiplying**", "17 × 23 = 391."], "gAAAA-reasoning"),
      ...message(1, "msg_1", ["17 × 23 = ", "391."]),
      completed({ reasoning: 40 }),
    ]),
  ],
  tool_call: [
    streamFixture([
      created(),
      ...reasoning(0, "rs_1", ["**Checking the weather**"], "gAAAA-tool"),
      ...weather(1, "1", "Seoul"),
      completed(),
    ]),
  ],
  parallel_tool_calls: [
    streamFixture([
      created(),
      ...weather(0, "1", "Seoul"),
      ...weather(1, "2", "Tokyo"),
      completed(),
    ]),
  ],
  same_producer_replay: [
    streamFixture([
      created(),
      ...reasoning(0, "rs_1", ["**Checking the weather**"], "gAAAA-replay"),
      ...message(1, "msg_1", ["Let me check."], "commentary"),
      ...weather(2, "1", "Seoul"),
      completed(),
    ]),
    streamFixture([created(), ...message(0, "msg_2", ["It is sunny in Seoul."]), completed()]),
  ],
  foreign_replay: [
    streamFixture([created(), ...message(0, "msg_1", ["Hello again!"]), completed()]),
  ],
  malformed_tool_input: [
    streamFixture([
      created(),
      ...call(0, "fc_1", "call_1", "get_weather", '{"city": "Seo'),
      incomplete("max_output_tokens"),
    ]),
  ],
  rate_limit: [
    errorFixture(429, "rate_limit_exceeded", "Rate limit reached. Please try again in 12s.", {
      "retry-after": "12",
    }),
  ],
  server_error: [
    errorFixture(503, "server_is_overloaded", "Our servers are currently overloaded."),
  ],
  overflow: [
    errorFixture(
      400,
      "context_length_exceeded",
      "Your input exceeds the context window of this model. Please adjust your input and try again.",
      {},
      "input",
    ),
  ],
  midstream_error: [
    streamFixture([
      created(),
      ...message(0, "msg_1", ["Hel"]).slice(0, 3),
      {
        type: "error",
        error: {
          type: "service_unavailable_error",
          code: "server_is_overloaded",
          message: "Our servers are currently overloaded. Please try again later.",
          param: null,
        },
      },
    ]),
  ],
};

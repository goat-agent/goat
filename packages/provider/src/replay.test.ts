import { expect, test } from "bun:test";
import type {
  AgentMessage,
  AgentPart,
  Message,
  Producer,
  Stop,
  ToolMessage,
  UserMessage,
} from "./message.ts";
import { replay } from "./replay.ts";

const claude: Producer = { provider: "anthropic", model: "claude-opus-5-5" };
const gpt: Producer = { provider: "openai", model: "gpt-6-sol" };
const done: Stop = { reason: "done" };

function user(text: string): UserMessage {
  return { role: "user", parts: [{ type: "text", text }] };
}

function agent(
  parts: readonly AgentPart[],
  producer: Producer = claude,
  stop?: Stop,
): AgentMessage {
  return { role: "agent", producer, parts, stop: stop ?? done };
}

function result(callId: string): ToolMessage {
  return {
    role: "tool",
    parts: [{ type: "tool_result", callId, parts: [{ type: "text", text: "ok" }], isError: false }],
  };
}

function call(id: string, input = "{}"): AgentPart {
  return { type: "tool_call", id, name: "read_file", input };
}

function replayed(messages: readonly Message[], target: Producer = claude): readonly Message[] {
  const outcome = replay(messages, target);
  if (!outcome.ok) {
    throw new Error(outcome.error.message);
  }
  return outcome.value;
}

test("keeps opaque data for the same producer", () => {
  const signed = agent([
    { type: "reasoning", text: "plan", opaque: { signature: "s1" } },
    { type: "text", text: "hi" },
  ]);
  expect(replayed([user("a"), signed, user("b")])).toEqual([user("a"), signed, user("b")]);
});

test("drops reasoning, opaque parts and opaque data for a foreign producer", () => {
  const foreign = agent(
    [
      { type: "reasoning", text: "plan", opaque: { signature: "s1" } },
      { type: "opaque", data: { block: "server_tool" } },
      { type: "text", text: "hi", opaque: { citation: 1 } },
    ],
    gpt,
  );
  expect(replayed([user("a"), foreign, user("b")])).toEqual([
    user("a"),
    agent([{ type: "text", text: "hi" }], gpt),
    user("b"),
  ]);
});

test("drops agent messages that ended with an error or were aborted", () => {
  const failed = agent([{ type: "text", text: "partial" }], claude, {
    reason: "error",
    error: { kind: "network", message: "reset" },
  });
  const aborted = agent([{ type: "text", text: "partial" }], claude, { reason: "aborted" });
  expect(replayed([user("a"), failed, aborted, user("b")])).toEqual([user("a"), user("b")]);
});

test("treats a same-producer message as foreign when it stopped at the length limit", () => {
  const length: Stop = { reason: "length" };
  const truncated = agent(
    [
      { type: "reasoning", text: "plan", opaque: { signature: "s1" } },
      { type: "text", text: "half" },
    ],
    claude,
    length,
  );
  expect(replayed([user("a"), truncated, user("b")])).toEqual([
    user("a"),
    agent([{ type: "text", text: "half" }], claude, length),
    user("b"),
  ]);
});

test("keeps a malformed call with its error result and treats the message as foreign", () => {
  const broken = agent([
    { type: "reasoning", text: "plan", opaque: { signature: "s1" } },
    call("c1", '{"path":'),
  ]);
  const failure: ToolMessage = {
    role: "tool",
    parts: [
      {
        type: "tool_result",
        callId: "c1",
        parts: [{ type: "text", text: "invalid input" }],
        isError: true,
      },
    ],
  };
  expect(replayed([user("a"), broken, failure])).toEqual([
    user("a"),
    agent([call("c1", '{"path":')]),
    failure,
  ]);
});

test("drops blank text everywhere except signed agent parts", () => {
  const image = {
    type: "media",
    mediaType: "image/png",
    source: { kind: "base64", data: "AA==" },
  } as const;
  const messages: readonly Message[] = [
    { role: "system", parts: [{ type: "text", text: " " }] },
    { role: "user", parts: [{ type: "text", text: "" }, image] },
    agent([call("c1")]),
    {
      role: "tool",
      parts: [
        { type: "tool_result", callId: "c1", parts: [{ type: "text", text: "" }], isError: false },
      ],
    },
    agent([
      { type: "text", text: "", opaque: { thoughtSignature: "g1" } },
      { type: "text", text: "  " },
      { type: "text", text: "done" },
    ]),
    user("next"),
  ];
  expect(replayed(messages)).toEqual([
    { role: "user", parts: [image] },
    agent([call("c1")]),
    { role: "tool", parts: [{ type: "tool_result", callId: "c1", parts: [], isError: false }] },
    agent([
      { type: "text", text: "", opaque: { thoughtSignature: "g1" } },
      { type: "text", text: "done" },
    ]),
    user("next"),
  ]);
});

test("moves messages that sit between a call and its results after the results", () => {
  const steer = user("also check b.ts");
  const note: Message = { role: "system", parts: [{ type: "text", text: "be brief" }] };
  const calls = agent([call("c1"), call("c2")]);
  expect(replayed([user("a"), calls, steer, result("c1"), note, result("c2")])).toEqual([
    user("a"),
    calls,
    result("c1"),
    result("c2"),
    steer,
    note,
  ]);
});

test("synthesizes missing results and drops orphan results", () => {
  const missing: ToolMessage = {
    role: "tool",
    parts: [
      {
        type: "tool_result",
        callId: "c1",
        parts: [{ type: "text", text: "No result was recorded for this call." }],
        isError: true,
      },
    ],
  };
  expect(replayed([user("a"), agent([call("c1")]), result("ghost"), user("b")])).toEqual([
    user("a"),
    agent([call("c1")]),
    missing,
    user("b"),
  ]);
});

test("drops messages left without parts", () => {
  const reasoningOnly = agent(
    [{ type: "reasoning", text: "plan", opaque: { signature: "s1" } }],
    gpt,
  );
  expect(replayed([user("a"), reasoningOnly, user("b")])).toEqual([user("a"), user("b")]);
});

test("rejects a request that ends with an agent message", () => {
  const outcome = replay([user("a"), agent([{ type: "text", text: "hi" }])], claude);
  expect(outcome.ok ? undefined : outcome.error.kind).toBe("invalid");
});

test("treats same-producer messages before the last user message as foreign when asked", () => {
  const early = agent([{ type: "text", text: "one", opaque: { id: "m1" } }]);
  const late = agent([call("c2"), { type: "text", text: "two", opaque: { id: "m2" } }]);
  const outcome = replay([user("a"), early, user("b"), late, result("c2")], claude, {
    foreign: "beforeLastUser",
  });
  expect(outcome.ok ? outcome.value : []).toEqual([
    user("a"),
    agent([{ type: "text", text: "one" }]),
    user("b"),
    late,
    result("c2"),
  ]);
});

test("treats every same-producer message as foreign when asked", () => {
  const signed = agent([{ type: "text", text: "one", opaque: { id: "m1" } }]);
  const outcome = replay([user("a"), signed, user("b")], claude, { foreign: "all" });
  expect(outcome.ok ? outcome.value : []).toEqual([
    user("a"),
    agent([{ type: "text", text: "one" }]),
    user("b"),
  ]);
});

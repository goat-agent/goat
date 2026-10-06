import { expect, test } from "bun:test";
import type { AgentMessage, AgentPart, Input, MediaPart, Message } from "@goat/provider";
import { weatherTool } from "@goat/provider/testing";
import { request, type Shape, wireId } from "./request.ts";

const producer = { provider: "openai", model: "gpt-6-luna" };
const shape: Shape = { namespaced: false, maxOutputTokens: true, summaries: true };

function user(text: string): Message {
  return { role: "user", parts: [{ type: "text", text }] };
}

function agent(parts: readonly AgentPart[]): AgentMessage {
  return { role: "agent", producer, parts, stop: { reason: "done" } };
}

function body(messages: readonly Message[], options: Partial<Input> = {}, given = shape) {
  return request("gpt-6-luna", { messages, ...options }, messages, given);
}

test("sends system messages as developer messages where they occur", () => {
  const sent = body([
    { role: "system", parts: [{ type: "text", text: "Be brief." }] },
    user("Hi"),
    agent([{ type: "text", text: "Hello." }]),
    { role: "system", parts: [{ type: "text", text: "Now answer in French." }] },
    user("Again"),
  ]);
  expect(sent["input"]).toEqual([
    { type: "message", role: "developer", content: [{ type: "input_text", text: "Be brief." }] },
    { type: "message", role: "user", content: [{ type: "input_text", text: "Hi" }] },
    { type: "message", role: "assistant", content: "Hello." },
    {
      type: "message",
      role: "developer",
      content: [{ type: "input_text", text: "Now answer in French." }],
    },
    { type: "message", role: "user", content: [{ type: "input_text", text: "Again" }] },
  ]);
  expect(sent).toMatchObject({ store: false, stream: true, reasoning: { summary: "auto" } });
});

const weatherCall: AgentPart = {
  type: "tool_call",
  id: "call_1",
  name: "get_weather",
  input: '{"city":"Seoul"}',
  opaque: { id: "fc_1", namespace: "functions" },
};

test("rebuilds its own reasoning and message items from their parts", () => {
  const sent = body([
    user("Weather?"),
    agent([
      { type: "reasoning", text: "**Plan**", opaque: { id: "rs_1" } },
      { type: "reasoning", text: "Call it.", opaque: { id: "rs_1", encrypted_content: "enc" } },
      { type: "text", text: "Checking.", opaque: { id: "msg_1", phase: "commentary" } },
    ]),
    user("Go on."),
  ]);
  expect(sent["input"]).toMatchObject([
    { role: "user" },
    {
      id: "rs_1",
      type: "reasoning",
      summary: [
        { type: "summary_text", text: "**Plan**" },
        { type: "summary_text", text: "Call it." },
      ],
      encrypted_content: "enc",
    },
    {
      id: "msg_1",
      phase: "commentary",
      type: "message",
      role: "assistant",
      status: "completed",
      content: [{ type: "output_text", text: "Checking.", annotations: [] }],
    },
    { role: "user" },
  ]);
});

test("replays its own calls with their item ids and pairs the results", () => {
  const sent = body([
    user("Weather?"),
    agent([weatherCall]),
    {
      role: "tool",
      parts: [
        {
          type: "tool_result",
          callId: "call_1",
          parts: [{ type: "text", text: "Sunny." }],
          isError: false,
        },
      ],
    },
  ]);
  expect(sent["input"]).toMatchObject([
    { role: "user" },
    {
      id: "fc_1",
      namespace: "functions",
      type: "function_call",
      call_id: "call_1",
      name: "get_weather",
      arguments: '{"city":"Seoul"}',
    },
    { type: "function_call_output", call_id: "call_1", output: "Sunny." },
  ]);
});

test("drops reasoning that cannot be replayed without its encrypted content", () => {
  const sent = body([
    user("Hi"),
    agent([
      { type: "reasoning", text: "Thought.", opaque: { id: "rs_1" } },
      { type: "text", text: "Hello." },
    ]),
    user("Again"),
  ]);
  expect(JSON.stringify(sent["input"])).not.toContain("rs_1");
});

test("sends media and mixed tool results as content arrays", () => {
  const image: MediaPart = {
    type: "media",
    mediaType: "image/png",
    source: { kind: "base64", data: "AAA" },
  };
  const sent = body([
    {
      role: "user",
      parts: [
        { type: "media", mediaType: "application/pdf", source: { kind: "base64", data: "UERG" } },
        { type: "media", mediaType: "text/csv", source: { kind: "url", url: "https://x.test/a" } },
      ],
    },
    agent([{ type: "tool_call", id: "toolu_01|abc", name: "shot", input: "{}" }]),
    {
      role: "tool",
      parts: [
        {
          type: "tool_result",
          callId: "toolu_01|abc",
          parts: [{ type: "text", text: "Here." }, image],
          isError: false,
        },
      ],
    },
  ]);
  const callId = wireId("toolu_01|abc");
  expect(sent["input"]).toEqual([
    {
      type: "message",
      role: "user",
      content: [
        { type: "input_file", filename: "file.pdf", file_data: "data:application/pdf;base64,UERG" },
        { type: "input_file", file_url: "https://x.test/a" },
      ],
    },
    { type: "function_call", call_id: callId, name: "shot", arguments: "{}" },
    {
      type: "function_call_output",
      call_id: callId,
      output: [
        { type: "input_text", text: "Here." },
        { type: "input_image", image_url: "data:image/png;base64,AAA", detail: "auto" },
      ],
    },
  ]);
});

test("keeps valid call ids and rewrites the rest within 64 characters", () => {
  expect(wireId("call_abc-1")).toBe("call_abc-1");
  const rewritten = wireId(`toolu_${"x".repeat(80)}|fc_1`);
  expect(rewritten).toMatch(/^[\w-]{1,64}$/u);
  expect(rewritten).toBe(wireId(`toolu_${"x".repeat(80)}|fc_1`));
  expect(wireId("")).toMatch(/^_[0-9a-f]{8}$/u);
});

test("asks for strict output and loose tools", () => {
  const schema = { type: "object", properties: { n: { type: "number" } } };
  const sent = body([user("Hi")], { tools: [weatherTool], outputSchema: schema, effort: "high" });
  expect(sent["tools"]).toEqual([
    {
      type: "function",
      name: weatherTool.name,
      description: weatherTool.description,
      parameters: weatherTool.inputSchema,
      strict: false,
    },
  ]);
  expect(sent["text"]).toEqual({
    format: { type: "json_schema", name: "output", schema, strict: true },
  });
  expect(sent["reasoning"]).toEqual({ effort: "high", summary: "auto" });
});

test("turns reasoning off without asking for summaries", () => {
  expect(body([user("Hi")], { effort: "none" })["reasoning"]).toEqual({ effort: "none" });
  const quiet = body([user("Hi")], {}, { ...shape, summaries: false });
  expect(quiet["reasoning"]).toBeUndefined();
});

test("hashes conversation ids that exceed the cache key limit", () => {
  const long = "c".repeat(80);
  const key = body([user("Hi")], {}, { ...shape, conversationId: long })["prompt_cache_key"];
  expect(key).toMatch(/^[0-9a-f]{64}$/u);
});

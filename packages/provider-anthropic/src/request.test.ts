import { expect, test } from "bun:test";
import type { AgentMessage, AgentPart, Input, Message } from "../provider/index.ts";
import { weatherTool } from "../provider/testing/index.ts";
import { catalogProfile, type Profile } from "./catalog.ts";
import { request, wireId } from "./request.ts";

const producer = { provider: "anthropic", model: "claude-opus-5-5" };

function profile(id: string): Profile {
  const found = catalogProfile(id);
  if (found === undefined) {
    throw new Error(`No catalog entry for ${id}.`);
  }
  return found;
}

function system(text: string): Message {
  return { role: "system", parts: [{ type: "text", text }] };
}

function user(text: string): Message {
  return { role: "user", parts: [{ type: "text", text }] };
}

function agent(parts: readonly AgentPart[]): AgentMessage {
  return { role: "agent", producer, parts, stop: { reason: "done" } };
}

function result(callId: string, text: string): Message {
  return {
    role: "tool",
    parts: [{ type: "tool_result", callId, parts: [{ type: "text", text }], isError: false }],
  };
}

function body(messages: readonly Message[], options: Partial<Input> = {}, id = "claude-opus-5-5") {
  return request(id, { messages, ...options }, messages, profile(id)).body;
}

test("caches the stable prefix for an hour and the rest automatically", () => {
  const sent = body([system("Be brief."), user("Hi")], { tools: [weatherTool] });
  expect(sent["system"]).toEqual([
    { type: "text", text: "Be brief.", cache_control: { type: "ephemeral", ttl: "1h" } },
  ]);
  expect(sent["tools"]).toEqual([
    {
      name: "get_weather",
      description: weatherTool.description,
      input_schema: weatherTool.inputSchema,
      eager_input_streaming: true,
      cache_control: { type: "ephemeral", ttl: "1h" },
    },
  ]);
  expect(sent["cache_control"]).toEqual({ type: "ephemeral" });
  expect(sent["stream"]).toBe(true);
  expect(sent["max_tokens"]).toBe(128_000);
});

test("sends later system messages natively where the model allows it", () => {
  const messages = [
    user("Hi"),
    agent([{ type: "text", text: "Hello." }]),
    system("Plan mode."),
    system("Be terse."),
    user("Go"),
  ];
  expect(body(messages)["messages"]).toEqual([
    { role: "user", content: [{ type: "text", text: "Hi" }] },
    { role: "assistant", content: [{ type: "text", text: "Hello." }] },
    { role: "user", content: [{ type: "text", text: "Go" }] },
    {
      role: "system",
      content: [
        { type: "text", text: "Plan mode." },
        { type: "text", text: "Be terse." },
      ],
    },
  ]);
});

test("folds later system messages into the user turn elsewhere", () => {
  const messages = [user("Hi"), system("Plan mode."), user("Go")];
  expect(body(messages, {}, "claude-sonnet-4-6")["messages"]).toEqual([
    {
      role: "user",
      content: [
        { type: "text", text: "Hi" },
        { type: "text", text: "Go" },
        { type: "text", text: "Plan mode." },
      ],
    },
  ]);
});

test("replays the agent turn faithfully and merges tool results", () => {
  const messages = [
    user("Weather?"),
    agent([
      { type: "reasoning", text: "Call it.", opaque: { signature: "sig" } },
      { type: "reasoning", text: "Unsigned." },
      { type: "opaque", data: { type: "redacted_thinking", data: "x" } },
      { type: "text", text: "Checking.", opaque: { citations: [] } },
      { type: "tool_call", id: "call.1", name: "get_weather", input: '{"city":"Seoul"}' },
      { type: "tool_call", id: "toolu_2", name: "get_weather", input: '{"city":' },
    ]),
    result("call.1", "Sunny."),
    result("toolu_2", "Rainy."),
  ];
  const foreignId = wireId("call.1");
  expect(foreignId).toMatch(/^call_1_[\da-f]{8}$/u);
  expect(body(messages)["messages"]).toEqual([
    { role: "user", content: [{ type: "text", text: "Weather?" }] },
    {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "Call it.", signature: "sig" },
        { type: "redacted_thinking", data: "x" },
        { type: "text", text: "Checking.", citations: [] },
        { type: "tool_use", id: foreignId, name: "get_weather", input: { city: "Seoul" } },
        { type: "tool_use", id: "toolu_2", name: "get_weather", input: {} },
      ],
    },
    {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: foreignId,
          content: [{ type: "text", text: "Sunny." }],
        },
        {
          type: "tool_result",
          tool_use_id: "toolu_2",
          content: [{ type: "text", text: "Rainy." }],
        },
      ],
    },
  ]);
});

test("maps effort to adaptive thinking and binds it where the model requires", () => {
  const plain = request(
    "claude-opus-5-5",
    { messages: [user("Hi")] },
    [user("Hi")],
    profile("claude-opus-5-5"),
  );
  expect(plain.body["thinking"]).toEqual({
    type: "adaptive",
    display: "summarized",
    block_binding: { prefix_mismatch_behavior: "drop_block" },
  });
  expect(plain.body["output_config"]).toEqual({ effort: "medium" });
  expect(plain.betas).toEqual(["thinking-binding-controls-2026-08-01"]);
  const off = request(
    "claude-opus-4-8",
    { messages: [user("Hi")], effort: "none" },
    [user("Hi")],
    profile("claude-opus-4-8"),
  );
  expect(off.body["thinking"]).toEqual({ type: "disabled" });
  expect(off.body["output_config"]).toBeUndefined();
  expect(off.betas).toEqual([]);
});

test("maps effort to a thinking budget on older models", () => {
  const haiku = "claude-haiku-4-5";
  expect(body([user("Hi")], { effort: "high" }, haiku)["thinking"]).toEqual({
    type: "enabled",
    budget_tokens: 32_768,
  });
  expect(body([user("Hi")], {}, haiku)["thinking"]).toEqual({ type: "disabled" });
  expect(body([user("Hi")], { effort: "high", maxOutputTokens: 2000 }, haiku)["thinking"]).toEqual({
    type: "disabled",
  });
});

test("requests structured output", () => {
  const schema = { type: "object", properties: {}, additionalProperties: false };
  expect(body([user("Hi")], { outputSchema: schema, effort: "low" })["output_config"]).toEqual({
    effort: "low",
    format: { type: "json_schema", schema },
  });
});

test("encodes media as images and documents", () => {
  const encoded = Buffer.from("plain words").toString("base64");
  const messages: readonly Message[] = [
    {
      role: "user",
      parts: [
        { type: "media", mediaType: "image/png", source: { kind: "base64", data: "iVBOR" } },
        {
          type: "media",
          mediaType: "image/webp",
          source: { kind: "url", url: "https://x.test/a.webp" },
        },
        {
          type: "media",
          mediaType: "application/pdf",
          source: { kind: "base64", data: "JVBER" },
          name: "spec.pdf",
        },
        { type: "media", mediaType: "text/plain", source: { kind: "base64", data: encoded } },
      ],
    },
  ];
  expect(body(messages)["messages"]).toEqual([
    {
      role: "user",
      content: [
        { type: "image", source: { type: "base64", media_type: "image/png", data: "iVBOR" } },
        { type: "image", source: { type: "url", url: "https://x.test/a.webp" } },
        {
          type: "document",
          source: { type: "base64", media_type: "application/pdf", data: "JVBER" },
          title: "spec.pdf",
        },
        {
          type: "document",
          source: { type: "text", media_type: "text/plain", data: "plain words" },
        },
      ],
    },
  ]);
});

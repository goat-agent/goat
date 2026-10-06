import { expect, test } from "bun:test";
import type { AgentMessage, Event, Message, Stop } from "@goat/provider";
import { conformance, type Fetch, type Fixture, recordedFetch } from "@goat/provider/testing";
import {
  errorFixture,
  finish,
  handwritten,
  start,
  streamFixture,
  text,
  toolUse,
} from "./fixtures.ts";
import recorded from "../fixtures/recorded.json";
import { apiKey, oauth } from "./provider.ts";

conformance({
  name: `anthropic (${recorded.model})`,
  model: recorded.model,
  effort: recorded.effort,
  offlineModelInfo: true,
  create: (fetch) => apiKey({ key: "test-key", fetch }),
  fixtures: { ...handwritten, ...recorded.fixtures },
});

const hello = streamFixture([start(), ...text(0, "Hi."), ...finish("end_turn")]);
const call = streamFixture([
  start(),
  ...toolUse(0, "toolu_1", "list_files", '{"path": ', '"/"}'),
  ...finish("tool_use"),
]);
const signatureRejection = errorFixture(
  400,
  "invalid_request_error",
  "messages.1.content.0: Invalid `signature` in `thinking` block",
);

function user(content: string): Message {
  return { role: "user", parts: [{ type: "text", text: content }] };
}

function token(): Promise<string> {
  return Promise.resolve("sk-ant-oat01-x");
}

function stalledToken(): Promise<string> {
  return new Promise<string>(() => {});
}

function failingToken(): Promise<string> {
  return Promise.reject(new Error("refresh failed"));
}

function agent(stop: Stop): AgentMessage {
  return {
    role: "agent",
    producer: { provider: "anthropic", model: "claude-opus-5-5" },
    parts: [
      { type: "reasoning", text: "Earlier thought.", opaque: { signature: "sig-old" } },
      { type: "text", text: "Earlier answer." },
    ],
    stop,
  };
}

function json(value: unknown): Fixture {
  return {
    status: 200,
    headers: { "content-type": "application/json" },
    chunks: [JSON.stringify(value)],
  };
}

async function stopOf(stream: AsyncIterable<Event>): Promise<Stop | undefined> {
  let stop: Stop | undefined;
  for await (const event of stream) {
    stop = event.type === "end" ? event.message.stop : stop;
  }
  return stop;
}

const conversation = [user("First."), agent({ reason: "done" }), user("Second.")];

test("resends without replayed reasoning when the API rejects it", async () => {
  const transport = recordedFetch([signatureRejection, hello]);
  const model = apiKey({ key: "k", fetch: transport.fetch }).model("claude-opus-5-5");
  expect(await stopOf(model.stream({ messages: conversation }))).toEqual({ reason: "done" });
  expect(transport.requests.map((request) => request.body.includes("sig-old"))).toEqual([
    true,
    false,
  ]);
});

test("gives up once stripping reasoning changes nothing", async () => {
  const transport = recordedFetch([signatureRejection, signatureRejection]);
  const model = apiKey({ key: "k", fetch: transport.fetch }).model("claude-opus-5-5");
  const stop = await stopOf(model.stream({ messages: conversation }));
  expect(stop?.reason === "error" && stop.error.kind).toBe("invalid");
  expect(transport.requests).toHaveLength(2);
});

test("leaves its own refused turns out of the request", async () => {
  const transport = recordedFetch([hello]);
  const model = apiKey({ key: "k", fetch: transport.fetch }).model("claude-opus-5-5");
  await stopOf(
    model.stream({ messages: [user("First."), agent({ reason: "refusal" }), user("Second.")] }),
  );
  expect(transport.requests[0]?.body).not.toContain("Earlier answer.");
});

test("lists live models with catalog details and falls back to the catalog", async () => {
  const live = json({
    data: [
      { id: "claude-opus-5-5", display_name: "Opus", max_input_tokens: 1, max_tokens: 1 },
      { id: "claude-future-6", display_name: "Future", max_input_tokens: 9, max_tokens: 3 },
      { id: "broken" },
    ],
    has_more: false,
  });
  const transport = recordedFetch([live]);
  const listed = await apiKey({ key: "k", fetch: transport.fetch }).models();
  const models = listed.ok ? listed.value : [];
  expect(models.map((model) => [model.id, model.contextWindow, model.price?.input])).toEqual([
    ["claude-opus-5-5", 1_000_000, 4],
    ["claude-future-6", 9, undefined],
  ]);
  expect(transport.requests[0]?.url).toBe("https://api.anthropic.com/v1/models?limit=1000");
  const offline = recordedFetch([errorFixture(401, "authentication_error", "no")]);
  const fallback = await apiKey({ key: "k", fetch: offline.fetch }).models();
  expect(fallback.ok ? fallback.value.length : 0).toBe(11);
});

test("asks the API once about models it does not know", async () => {
  const transport = recordedFetch([
    json({ id: "claude-future-6", display_name: "Future", max_input_tokens: 9, max_tokens: 3 }),
  ]);
  const model = apiKey({ key: "k", fetch: transport.fetch, baseUrl: "https://proxy.test/" }).model(
    "claude-future-6",
  );
  expect((await model.info()).ok).toBe(true);
  expect((await model.info()).ok).toBe(true);
  expect(transport.requests.map((request) => request.url)).toEqual([
    "https://proxy.test/v1/models/claude-future-6",
  ]);
});

test("sends OAuth tokens as bearer tokens with the OAuth beta", async () => {
  const seen: Headers[] = [];
  const transport = recordedFetch([hello]);
  const fetch: Fetch = async (input, init) => {
    seen.push(new Headers(init?.headers));
    return transport.fetch(input, init);
  };
  await stopOf(
    oauth({ token, fetch })
      .model("claude-opus-5-5")
      .stream({ messages: [user("Hi")] }),
  );
  expect(seen[0]?.get("authorization")).toBe("Bearer sk-ant-oat01-x");
  expect(seen[0]?.get("anthropic-beta")).toBe(
    "oauth-2025-04-20,thinking-binding-controls-2026-08-01",
  );
  expect(seen[0]?.get("anthropic-version")).toBe("2023-06-01");
});

test("reports a failing token source as an auth error without sending", async () => {
  const transport = recordedFetch([]);
  const model = oauth({ token: failingToken, fetch: transport.fetch }).model("claude-opus-5-5");
  expect(await stopOf(model.stream({ messages: [user("Hi")] }))).toEqual({
    reason: "error",
    error: { kind: "auth", message: "refresh failed" },
  });
  expect(transport.requests).toHaveLength(0);
});

test("stops at an abort even when the rest of the response already arrived", async () => {
  const transport = recordedFetch([call], "whole");
  const controller = new AbortController();
  const stream = apiKey({ key: "k", fetch: transport.fetch })
    .model("claude-opus-5-5")
    .stream({ messages: [user("Hi")] }, { signal: controller.signal });
  let stop: Stop | undefined;
  for await (const event of stream) {
    if (event.type === "part_delta") {
      controller.abort();
    }
    stop = event.type === "end" ? event.message.stop : stop;
  }
  expect(stop).toEqual({ reason: "aborted" });
});

test("does not wait for a token source after an abort", async () => {
  const controller = new AbortController();
  const stream = oauth({ token: stalledToken, fetch: recordedFetch([]).fetch })
    .model("claude-opus-5-5")
    .stream({ messages: [user("Hi")] }, { signal: controller.signal });
  setTimeout(() => {
    controller.abort();
  }, 10);
  expect(await stopOf(stream)).toEqual({ reason: "aborted" });
});

test("refuses credentials that cannot be sent without echoing them", async () => {
  const transport = recordedFetch([]);
  const model = apiKey({ key: "sk-secret\nx", fetch: transport.fetch }).model("claude-opus-5-5");
  const stop = await stopOf(model.stream({ messages: [user("Hi")] }));
  expect(stop?.reason === "error" && stop.error.kind).toBe("auth");
  expect(JSON.stringify(stop)).not.toContain("sk-secret");
  expect(transport.requests).toHaveLength(0);
});

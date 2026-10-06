import { expect, test } from "bun:test";
import type { AgentMessage, Event, Message, Stop } from "@goat/provider";
import { type Fixture, recordedFetch } from "@goat/provider/testing";
import { completed, created, errorFixture, message, streamFixture } from "./fixtures.ts";
import { apiKey, chatgpt } from "./provider.ts";

const model = "gpt-6-luna";
const hello = streamFixture([created(), ...message(0, "msg_9", ["Hi."]), completed()]);

function user(text: string): Message {
  return { role: "user", parts: [{ type: "text", text }] };
}

function json(value: unknown): Fixture {
  return {
    status: 200,
    headers: { "content-type": "application/json" },
    chunks: [JSON.stringify(value)],
  };
}

function sentBody(transport: { readonly requests: readonly { readonly body: string }[] }): unknown {
  return JSON.parse(transport.requests[0]?.body ?? "{}");
}

async function endOf(stream: AsyncIterable<Event>): Promise<AgentMessage | undefined> {
  let last: AgentMessage | undefined;
  for await (const event of stream) {
    last = event.type === "end" ? event.message : last;
  }
  return last;
}

async function stopOf(stream: AsyncIterable<Event>): Promise<Stop | undefined> {
  return (await endOf(stream))?.stop;
}

test("prices API key turns and leaves plan turns unpriced", async () => {
  const priced = await endOf(
    apiKey({ key: "k", fetch: recordedFetch([hello]).fetch })
      .model(model)
      .stream({ messages: [user("Hi")] }),
  );
  expect(priced?.usage?.cost).toBeCloseTo((12 * 0.1 + 20 * 0.5) / 1_000_000);
  const plain = await endOf(
    chatgpt({ token: "t", fetch: recordedFetch([hello]).fetch })
      .model(model)
      .stream({ messages: [user("Hi")] }),
  );
  expect(plain?.usage).toEqual({
    input: { total: 12, cacheRead: 0, cacheWrite: 0 },
    output: { total: 20, reasoning: 0 },
  });
  expect((await chatgpt({ token: "t" }).model(model).info()).ok).toBe(true);
});

test("sends plan requests within Sign in with ChatGPT limits", async () => {
  const transport = recordedFetch([hello]);
  const tool = {
    type: "function" as const,
    name: "read",
    description: "Read a file.",
    inputSchema: { type: "object" },
  };
  await stopOf(
    chatgpt({ token: "t", fetch: transport.fetch })
      .model(model)
      .stream({ messages: [user("Hi")], tools: [tool], maxOutputTokens: 100 }),
  );
  const body = sentBody(transport);
  expect(body).not.toHaveProperty("max_output_tokens");
  expect(body).toHaveProperty("tools", [
    {
      type: "namespace",
      name: "functions",
      description: "Functions available in this conversation.",
      tools: [
        {
          type: "function",
          name: "read",
          description: "Read a file.",
          parameters: { type: "object" },
          strict: false,
        },
      ],
    },
  ]);
});

test("sends API key requests with the output limit, tier and cache key", async () => {
  const transport = recordedFetch([hello]);
  await stopOf(
    apiKey({ key: "k", fetch: transport.fetch, serviceTier: "flex" })
      .model(model)
      .stream({ messages: [user("Hi")], maxOutputTokens: 100 }, { conversationId: "conv-1" }),
  );
  expect(sentBody(transport)).toMatchObject({
    max_output_tokens: 100,
    service_tier: "flex",
    prompt_cache_key: "conv-1",
    store: false,
    stream: true,
  });
  expect(transport.requests[0]?.url).toBe("https://api.openai.com/v1/responses");
});

test("lists account models the catalog describes and falls back to the catalog", async () => {
  const live = json({ object: "list", data: [{ id: "gpt-6-luna" }, { id: "dall-e-3" }] });
  const listed = await apiKey({ key: "k", fetch: recordedFetch([live]).fetch }).models();
  expect(listed.ok && listed.value.map((info) => info.id)).toEqual(["gpt-6-luna"]);
  const failing = recordedFetch([errorFixture(500, "server_error", "down")]);
  const fallback = await apiKey({ key: "k", fetch: failing.fetch }).models();
  expect(fallback.ok ? fallback.value.length : 0).toBe(10);
  const plans = json({
    models: [
      { slug: "gpt-6.1-sol", display_name: "GPT-6.1 Sol", visibility: "list" },
      { slug: "gpt-reserve", display_name: "Reserve", visibility: "hide" },
    ],
  });
  const offered = await chatgpt({ token: "t", fetch: recordedFetch([plans]).fetch }).models();
  expect(offered.ok && offered.value.map((info) => [info.id, info.price])).toEqual([
    ["gpt-6.1-sol", undefined],
  ]);
});

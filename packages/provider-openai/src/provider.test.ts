import { expect, test } from "bun:test";
import type { AgentMessage, Event, Message, Stop } from "@goat/provider";
import { conformance, type Fetch, recordedFetch } from "@goat/provider/testing";
import api from "../fixtures/api.json";
import auth from "../fixtures/auth.json";
import plan from "../fixtures/chatgpt.json";
import {
  completed,
  created,
  errorFixture,
  failed,
  handwritten,
  message,
  streamFixture,
} from "./fixtures.ts";
import { apiKey, chatgpt } from "./provider.ts";

conformance({
  name: `openai api key (${api.model})`,
  model: api.model,
  effort: api.effort,
  offlineModelInfo: true,
  create: (fetch) => apiKey({ key: "test-key", fetch }),
  fixtures: { ...handwritten, ...auth.fixtures, ...api.fixtures },
});

conformance({
  name: `openai chatgpt (${plan.model})`,
  model: plan.model,
  effort: plan.effort,
  offlineModelInfo: true,
  create: (fetch) => chatgpt({ token: "test-token", fetch }),
  fixtures: { ...handwritten, ...auth.fixtures, ...plan.fixtures },
});

const model = "gpt-6-luna";
const hello = streamFixture([created(), ...message(0, "msg_9", ["Hi."]), completed()]);
const encryptedRejection = errorFixture(
  400,
  "invalid_encrypted_content",
  "The encrypted content gAAA...oRk= could not be verified. Reason: Encrypted content could not be decrypted or parsed.",
);
const summaryRejection = errorFixture(
  400,
  "unsupported_value",
  "Your organization must be verified to generate reasoning summaries.",
  {},
  "reasoning.summary",
);

function failingToken(): Promise<string> {
  return Promise.reject(new Error("refresh failed"));
}

function stalledToken(): Promise<string> {
  return new Promise<string>(() => {});
}

function user(text: string): Message {
  return { role: "user", parts: [{ type: "text", text }] };
}

function agent(stop: Stop): AgentMessage {
  return {
    role: "agent",
    producer: { provider: "openai", model },
    parts: [
      {
        type: "reasoning",
        text: "Earlier.",
        opaque: { id: "rs_old", encrypted_content: "enc-old" },
      },
      { type: "text", text: "Earlier answer.", opaque: { id: "msg_old", phase: "final_answer" } },
    ],
    stop,
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

test("resends without replayed reasoning when OpenAI rejects it", async () => {
  const transport = recordedFetch([encryptedRejection, hello]);
  const subject = apiKey({ key: "k", fetch: transport.fetch }).model(model);
  expect(await stopOf(subject.stream({ messages: conversation }))).toEqual({ reason: "done" });
  expect(transport.requests.map((request) => request.body.includes("enc-old"))).toEqual([
    true,
    false,
  ]);
});

test("treats a rejection inside the stream like a rejected request", async () => {
  const rejected = streamFixture([
    created(),
    failed("invalid_encrypted_content", "The encrypted content could not be verified."),
  ]);
  const transport = recordedFetch([rejected, hello]);
  const subject = apiKey({ key: "k", fetch: transport.fetch }).model(model);
  const events: Event[] = [];
  for await (const event of subject.stream({ messages: conversation })) {
    events.push(event);
  }
  expect(events.filter((event) => event.type === "end")).toHaveLength(1);
  expect(events.at(-1)?.type === "end" && events.at(-1)).toMatchObject({
    message: { stop: { reason: "done" } },
  });
  expect(transport.requests).toHaveLength(2);
});

test("stops asking for summaries once the organization cannot have them", async () => {
  const transport = recordedFetch([summaryRejection, hello, hello]);
  const provider = apiKey({ key: "k", fetch: transport.fetch });
  await stopOf(provider.model(model).stream({ messages: [user("Hi")] }));
  await stopOf(provider.model(model).stream({ messages: [user("Again")] }));
  expect(transport.requests.map((request) => request.body.includes('"summary"'))).toEqual([
    true,
    false,
    false,
  ]);
});

test("leaves its own refused turns out of the request", async () => {
  const transport = recordedFetch([hello]);
  const subject = apiKey({ key: "k", fetch: transport.fetch }).model(model);
  await stopOf(
    subject.stream({ messages: [user("First."), agent({ reason: "refusal" }), user("Second.")] }),
  );
  expect(transport.requests[0]?.body).not.toContain("Earlier answer.");
});

test("streams models the catalog does not know without checking them first", async () => {
  const transport = recordedFetch([hello]);
  const unknown = apiKey({ key: "k", fetch: transport.fetch }).model("gpt-7-preview");
  expect((await unknown.info()).ok).toBe(false);
  const stop = await stopOf(unknown.stream({ messages: [user("Hi")], effort: "ultra" }));
  expect(stop).toEqual({ reason: "done" });
});

test("reports a stream that ends without a terminal event as a network error", async () => {
  const cut = streamFixture([created(), ...message(0, "msg_1", ["Hel"]).slice(0, 3)]);
  const stop = await stopOf(
    apiKey({ key: "k", fetch: recordedFetch([cut]).fetch })
      .model(model)
      .stream({ messages: [user("Hi")] }),
  );
  expect(stop?.reason === "error" && stop.error.kind).toBe("network");
});

test("reports a failing token source as an auth error without sending", async () => {
  const transport = recordedFetch([]);
  const stop = await stopOf(
    chatgpt({ token: failingToken, fetch: transport.fetch })
      .model(model)
      .stream({ messages: [user("Hi")] }),
  );
  expect(stop).toEqual({ reason: "error", error: { kind: "auth", message: "refresh failed" } });
  expect(transport.requests).toHaveLength(0);
});

test("does not wait for a token source after an abort", async () => {
  const controller = new AbortController();
  const stream = chatgpt({ token: stalledToken, fetch: recordedFetch([]).fetch })
    .model(model)
    .stream({ messages: [user("Hi")] }, { signal: controller.signal });
  setTimeout(() => {
    controller.abort();
  }, 10);
  expect(await stopOf(stream)).toEqual({ reason: "aborted" });
});

test("refuses credentials that cannot be sent without echoing them", async () => {
  const transport = recordedFetch([]);
  const subject = apiKey({ key: "sk-secret\nx", fetch: transport.fetch }).model(model);
  const stop = await stopOf(subject.stream({ messages: [user("Hi")] }));
  expect(stop?.reason === "error" && stop.error.kind).toBe("auth");
  expect(JSON.stringify(stop)).not.toContain("sk-secret");
  expect(transport.requests).toHaveLength(0);
});

test("sends the bearer token to the configured base URL", async () => {
  const seen: Headers[] = [];
  const transport = recordedFetch([hello]);
  const fetch: Fetch = async (input, init) => {
    seen.push(new Headers(init?.headers));
    return transport.fetch(input, init);
  };
  await stopOf(
    chatgpt({ token: () => Promise.resolve("tok"), fetch, baseUrl: "https://proxy.test/v1/" })
      .model(model)
      .stream({ messages: [user("Hi")] }),
  );
  expect(seen[0]?.get("authorization")).toBe("Bearer tok");
  expect(transport.requests[0]?.url).toBe("https://proxy.test/v1/responses");
});

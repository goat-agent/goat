import { expect, test } from "bun:test";
import { type Chunking, type Fixture, recordedFetch, recordFetch } from "./fetch.ts";

const fixture: Fixture = {
  status: 200,
  headers: { "content-type": "text/event-stream" },
  chunks: ['data: {"text":"안녕"}\n\n', 'data: {"text":"하세요"}\n\n'],
};

async function chunksOf(response: Response): Promise<readonly number[]> {
  const sizes: number[] = [];
  const stream: ReadableStream<Uint8Array> = response.body ?? new ReadableStream();
  for await (const bytes of stream) {
    sizes.push(bytes.byteLength);
  }
  return sizes;
}

test("serves the same body for every chunking", async () => {
  const bodies = await Promise.all(
    (["recorded", "bytes", "whole"] as const).map(async (chunking: Chunking) => {
      const response = await recordedFetch([fixture], chunking).fetch("https://example.test/v1");
      return response.text();
    }),
  );
  expect(new Set(bodies).size).toBe(1);
  expect(bodies[0]).toBe(fixture.chunks.join(""));
});

test("splits the body by recorded chunks, single bytes, or one piece", async () => {
  const total = new TextEncoder().encode(fixture.chunks.join("")).byteLength;
  const recorded = await chunksOf(
    await recordedFetch([fixture], "recorded").fetch("https://example.test/v1"),
  );
  const bytes = await chunksOf(
    await recordedFetch([fixture], "bytes").fetch("https://example.test/v1"),
  );
  const whole = await chunksOf(
    await recordedFetch([fixture], "whole").fetch("https://example.test/v1"),
  );
  expect(recorded).toHaveLength(2);
  expect(bytes).toHaveLength(total);
  expect(whole).toEqual([total]);
});

test("records each request and fails when no response is left", async () => {
  const transport = recordedFetch([fixture]);
  await transport.fetch("https://example.test/v1", { method: "POST", body: '{"a":1}' });
  expect(transport.requests).toEqual([
    { url: "https://example.test/v1", method: "POST", body: '{"a":1}', signal: undefined },
  ]);
  const failure = await transport.fetch("https://example.test/v1").then(() => "resolved", String);
  expect(failure).toBe("Error: No recorded response for request 2.");
});

test("errors the body when the request is aborted", async () => {
  const controller = new AbortController();
  const response = await recordedFetch([fixture], "bytes").fetch("https://example.test/v1", {
    signal: controller.signal,
  });
  controller.abort();
  const outcome = await response.text().then(
    () => "resolved",
    () => "rejected",
  );
  expect(outcome).toBe("rejected");
});

test("records responses that replay identically", async () => {
  const recording = recordFetch(recordedFetch([fixture]).fetch);
  const response = await recording.fetch("https://example.test/v1");
  const text = await response.text();
  const [recorded] = await recording.fixtures();
  expect(text).toBe(fixture.chunks.join(""));
  expect(recorded?.chunks.join("")).toBe(text);
  expect(recorded?.headers).toEqual({ "content-type": "text/event-stream" });
});

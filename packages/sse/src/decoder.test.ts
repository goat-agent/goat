import { expect, test } from "bun:test";
import { Decoder, decode, type ServerSentEvent } from "./decoder.ts";

function pushAll(...chunks: readonly string[]): readonly ServerSentEvent[] {
  const decoder = new Decoder();
  return chunks.flatMap((chunk) => decoder.push(chunk));
}

function message(data: string, id = ""): ServerSentEvent {
  return { type: "message", data, id };
}

function streamOf(chunks: readonly Uint8Array[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) {
        controller.enqueue(chunk);
      }
      controller.close();
    },
  });
}

async function collect(
  events: AsyncIterable<ServerSentEvent>,
): Promise<readonly ServerSentEvent[]> {
  const collected: ServerSentEvent[] = [];
  for await (const event of events) {
    collected.push(event);
  }
  return collected;
}

test("joins data lines with line feeds", () => {
  expect(pushAll("data: YHOO\ndata: +2\ndata: 10\n\n")).toEqual([message("YHOO\n+2\n10")]);
});

test("keeps the last event id across events", () => {
  const stream =
    ": test stream\n\ndata: first event\nid: 1\n\ndata:second event\nid\n\ndata:  third event\n\n";
  expect(pushAll(stream)).toEqual([
    message("first event", "1"),
    message("second event"),
    message(" third event"),
  ]);
});

test("dispatches empty data but not events without data", () => {
  expect(pushAll("data\n\ndata\ndata\n\ndata:\n", "\nevent: idle\n\n")).toEqual([
    message(""),
    message("\n"),
    message(""),
  ]);
});

test("removes only one leading space from values", () => {
  expect(pushAll("data:test\n\ndata: test\n\ndata:  test\n\n")).toEqual([
    message("test"),
    message("test"),
    message(" test"),
  ]);
});

test("uses the event field as the type and resets it after dispatch", () => {
  expect(pushAll("event: add\ndata: 73857293\n\ndata: plain\n\n")).toEqual([
    { type: "add", data: "73857293", id: "" },
    message("plain"),
  ]);
});

test("accepts every line ending", () => {
  expect(pushAll("data: a\r\ndata: b\rdata: c\n\r\n")).toEqual([message("a\nb\nc")]);
});

test("treats a carriage return and line feed split across pushes as one line break", () => {
  expect(pushAll("data: a\r", "\ndata: b\r", "\n\r", "\n")).toEqual([message("a\nb")]);
});

test("decodes the same events however the text is split", () => {
  const stream = "\uFEFFevent: add\r\nid: 7\rdata: one\r\ndata:two\n\r\n: note\ndata: three\r\r";
  const whole = pushAll(stream);
  const units = Array.from({ length: stream.length }, (_, index) => stream.charAt(index));
  expect(pushAll(...units)).toEqual(whole);
  expect(whole).toEqual([{ type: "add", data: "one\ntwo", id: "7" }, message("three", "7")]);
});

test("strips only one leading byte order mark", () => {
  expect(pushAll("", "\uFEFFdata: a\n\n")).toEqual([message("a")]);
  expect(pushAll("\uFEFF\uFEFFdata: a\n\n")).toEqual([]);
});

test("ignores ids containing NULL", () => {
  expect(pushAll("id: 1\n\nid: 2\0\ndata: a\n\n")).toEqual([message("a", "1")]);
});

test("accepts only ASCII digit retry values", () => {
  const decoder = new Decoder();
  decoder.push("retry: 3000\n");
  expect(decoder.retry).toBe(3000);
  decoder.push("retry: 1.5\nretry: -1\nretry: \nretry: 1e3\n");
  expect(decoder.retry).toBe(3000);
});

test("ignores unknown fields", () => {
  expect(pushAll("foo: bar\nData: x\ndata: a\n\n")).toEqual([message("a")]);
});

test("discards an incomplete final event", async () => {
  const bytes = new TextEncoder().encode("data: a\n\ndata: b\n");
  expect(await collect(decode(streamOf([bytes])))).toEqual([message("a")]);
});

test("decodes characters split across byte chunks", async () => {
  const bytes = new TextEncoder().encode("data: 염소\n\n");
  const chunks = Array.from(bytes, (byte) => Uint8Array.of(byte));
  expect(await collect(decode(streamOf(chunks)))).toEqual([message("염소")]);
});

test("cancels the source when iteration stops early", async () => {
  const encoder = new TextEncoder();
  let cancelled = false;
  const source = new ReadableStream<Uint8Array>({
    pull(controller) {
      controller.enqueue(encoder.encode("data: tick\n\n"));
    },
    cancel() {
      cancelled = true;
    },
  });
  for await (const event of decode(source)) {
    expect(event).toEqual(message("tick"));
    break;
  }
  expect(cancelled).toBe(true);
});

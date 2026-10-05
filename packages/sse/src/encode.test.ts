import { expect, test } from "bun:test";
import { Decoder, type ServerSentEvent } from "./decoder.ts";
import { encode } from "./encode.ts";

test("encodes every field before the data", () => {
  expect(encode({ type: "add", id: "7", retry: 3000, data: "one" })).toBe(
    "event: add\nid: 7\nretry: 3000\ndata: one\n\n",
  );
});

test("splits data on every line ending", () => {
  expect(encode({ data: "a\r\nb\rc\nd" })).toBe("data: a\ndata: b\ndata: c\ndata: d\n\n");
});

test("round trips through the decoder", () => {
  const decoder = new Decoder();
  const events: readonly ServerSentEvent[] = [
    { type: "delta", data: '{"text":"염소"}', id: "1" },
    { type: "message", data: "\n line\n\n", id: "1" },
    { type: "message", data: "", id: "" },
  ];
  const decoded: readonly ServerSentEvent[] = events.flatMap((event) =>
    decoder.push(encode(event)),
  );
  expect(decoded).toEqual(events);
});

test("rejects fields the decoder would misread", () => {
  expect(() => encode({ type: "a\nb", data: "" })).toThrow(RangeError);
  expect(() => encode({ id: "a\rb", data: "" })).toThrow(RangeError);
  expect(() => encode({ id: "a\0b", data: "" })).toThrow(RangeError);
  expect(() => encode({ retry: -1, data: "" })).toThrow(RangeError);
  expect(() => encode({ retry: 1.5, data: "" })).toThrow(RangeError);
});

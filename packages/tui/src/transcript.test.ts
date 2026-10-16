import { expect, test } from "bun:test";
import type { ScreenEvent } from "./event.ts";
import type { AgentMessage } from "@goat/provider";
import { emptyTranscript, reduce, type Transcript } from "./transcript.ts";

const reply: AgentMessage = {
  role: "agent",
  producer: { provider: "test", model: "scripted" },
  parts: [],
  stop: { reason: "done" },
};

function play(events: readonly ScreenEvent[]): Transcript {
  return events.reduce((state, event) => reduce(state, event), emptyTranscript);
}

test("adds the user message and marks the turn running", () => {
  const state = play([
    { type: "user", message: { role: "user", parts: [{ type: "text", text: "hi" }] } },
  ]);
  expect(state.running).toBe(true);
  expect(state.blocks).toEqual([{ id: 1, version: 0, kind: "user", text: "hi" }]);
});

test("streams text into one block and finishes it with the final part", () => {
  const state = play([
    { type: "part_start", index: 0, part: { type: "text", text: "" } },
    { type: "part_delta", index: 0, text: "Hel" },
    { type: "part_delta", index: 0, text: "lo" },
  ]);
  expect(state.blocks[0]).toMatchObject({ kind: "text", text: "Hello", streaming: true });
  const finished = reduce(state, {
    type: "part_end",
    index: 0,
    part: { type: "text", text: "Hello!" },
  });
  expect(finished.blocks[0]).toMatchObject({ kind: "text", text: "Hello!", streaming: false });
  expect(finished.blocks[0]?.version).toBeGreaterThan(state.blocks[0]?.version ?? 0);
});

test("attaches a tool result to its call", () => {
  const state = play([
    {
      type: "part_start",
      index: 0,
      part: { type: "tool_call", id: "c1", name: "bash", input: "" },
    },
    {
      type: "part_end",
      index: 0,
      part: { type: "tool_call", id: "c1", name: "bash", input: '{"command":"ls"}' },
    },
    { type: "message", message: { ...reply, stop: { reason: "done" } } },
    {
      type: "tool_result",
      result: {
        type: "tool_result",
        callId: "c1",
        parts: [{ type: "text", text: "a\nexit 0" }],
        isError: false,
      },
    },
  ]);
  expect(state.blocks[0]).toMatchObject({
    kind: "tool",
    input: '{"command":"ls"}',
    result: { text: "a\nexit 0", isError: false },
  });
});

test("starts new blocks for the parts of the next turn", () => {
  const state = play([
    { type: "part_start", index: 0, part: { type: "text", text: "one" } },
    { type: "message", message: reply },
    { type: "part_start", index: 0, part: { type: "text", text: "two" } },
    { type: "part_delta", index: 0, text: "!" },
  ]);
  expect(state.blocks.map((block) => (block.kind === "text" ? block.text : ""))).toEqual([
    "one",
    "two!",
  ]);
});

test("ends the turn and explains stops other than done", () => {
  const stopped = play([{ type: "end", stop: { reason: "aborted" } }]);
  expect(stopped.running).toBe(false);
  expect(stopped.blocks).toEqual([
    { id: 1, version: 0, kind: "notice", text: "Stopped.", tone: "dim" },
  ]);
  const failed = play([
    { type: "end", stop: { reason: "error", error: { kind: "auth", message: "Sign in again." } } },
  ]);
  expect(failed.blocks[0]).toMatchObject({ kind: "notice", text: "Sign in again.", tone: "error" });
  expect(play([{ type: "end", stop: { reason: "done" } }]).blocks).toEqual([]);
});

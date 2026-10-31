import { describe, expect, test } from "vitest";
import type { Message } from "@/shared/api/client";
import { type Conversation, withMessage } from "./model";

function message(id: number, text = `m${id}`): Message {
  return { id, role: "assistant", text, createdAt: "2026-10-31T00:00:00Z" };
}

function conversation(...pages: number[][]): Conversation {
  return {
    pages: pages.map((page) => ({ messages: page.map((id) => message(id)) })),
    pageParams: pages.map(() => undefined),
  };
}

function ids(value: Conversation | undefined): number[][] {
  return value?.pages.map((page) => page.messages.map((item) => item.id)) ?? [];
}

describe("withMessage", () => {
  test("appends a new message to the newest page", () => {
    expect(ids(withMessage(conversation([1, 2], [3, 4]), message(5)))).toEqual([
      [1, 2],
      [3, 4, 5],
    ]);
  });

  test("keeps one copy when the stream repeats a message after reconnecting", () => {
    const repeated = withMessage(conversation([1, 2, 3]), message(3, "edited"));
    expect(ids(repeated)).toEqual([[1, 2, 3]]);
    expect(repeated?.pages[0]?.messages[2]?.text).toBe("edited");
  });

  test("puts a late message in order", () => {
    expect(ids(withMessage(conversation([1, 2, 5]), message(4)))).toEqual([[1, 2, 4, 5]]);
  });

  test("starts a page when the conversation was empty", () => {
    expect(ids(withMessage(conversation(), message(1)))).toEqual([[1]]);
  });

  test("waits for the first page before adding anything", () => {
    expect(withMessage(undefined, message(1))).toBeUndefined();
  });
});

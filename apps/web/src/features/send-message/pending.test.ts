import { describe, expect, test } from "vitest";
import type { Message } from "@/shared/api/client";
import { type Pending, stillPending } from "./pending";

function pending(localId: string, text: string, after: number): Pending {
  return { localId, text, files: [], after, state: "sent" };
}

function user(id: number, text: string): Message {
  return { id, role: "user", text, createdAt: "2026-10-31T00:00:00Z" };
}

describe("stillPending", () => {
  test("hides a pending message once the server's copy arrives", () => {
    expect(stillPending([pending("a", "안녕", 3)], [user(4, "안녕")])).toEqual([]);
  });

  test("ignores an older message with the same text", () => {
    const item = pending("a", "안녕", 3);
    expect(stillPending([item], [user(2, "안녕")])).toEqual([item]);
  });

  test("lets each copy hide only one of two identical sends", () => {
    const first = pending("a", "다시", 1);
    const second = pending("b", "다시", 1);
    expect(stillPending([first, second], [user(2, "다시")])).toEqual([second]);
  });
});

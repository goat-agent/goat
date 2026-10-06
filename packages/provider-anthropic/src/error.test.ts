import { expect, test } from "bun:test";
import { eventError, isOpaqueRejection, responseError } from "./error.ts";

function body(type: string, message: string, code?: string): string {
  const details = code === undefined ? {} : { details: { error_code: code } };
  return JSON.stringify({ type: "error", error: { type, message, ...details } });
}

function kindOf(status: number, text: string, headers: Readonly<Record<string, string>> = {}) {
  return responseError(status, new Headers(headers), text).kind;
}

test("maps statuses to error kinds", () => {
  expect(kindOf(401, body("authentication_error", "invalid key"))).toBe("auth");
  expect(kindOf(403, body("permission_error", "forbidden"))).toBe("auth");
  expect(kindOf(402, body("billing_error", "pay"))).toBe("quota");
  expect(kindOf(408, "")).toBe("unavailable");
  expect(kindOf(409, body("conflict_error", "busy"))).toBe("unavailable");
  expect(kindOf(413, body("request_too_large", "too large"))).toBe("overflow");
  expect(kindOf(429, body("rate_limit_error", "slow down"))).toBe("rate_limit");
  expect(kindOf(529, body("overloaded_error", "Overloaded"))).toBe("unavailable");
  expect(kindOf(404, body("not_found_error", "model: nope"))).toBe("invalid");
});

test("reads limits that do not lift soon as quota", () => {
  expect(kindOf(429, body("rate_limit_error", "cap", "enforced_spend_limit_reached"))).toBe(
    "quota",
  );
  expect(
    kindOf(400, body("invalid_request_error", "You have reached your specified API usage limits.")),
  ).toBe("quota");
  expect(kindOf(400, body("invalid_request_error", "Your credit balance is too low."))).toBe(
    "quota",
  );
  expect(kindOf(400, body("invalid_request_error", "You're out of extra usage."))).toBe("quota");
  const reset = String(Math.floor(Date.now() / 1000) + 3600);
  const limited = responseError(
    429,
    new Headers({
      "anthropic-ratelimit-unified-status": "rejected",
      "anthropic-ratelimit-unified-reset": reset,
    }),
    body("rate_limit_error", "limit"),
  );
  expect(limited.kind).toBe("quota");
  expect(limited.retryAfter).toBeGreaterThan(3_590_000);
});

test("reads overflow and other rejections from the message", () => {
  expect(
    kindOf(
      400,
      body("invalid_request_error", "prompt is too long: 1000512 tokens > 1000000 maximum"),
    ),
  ).toBe("overflow");
  expect(kindOf(400, body("invalid_request_error", "messages: roles must alternate"))).toBe(
    "invalid",
  );
});

function after(headers: Readonly<Record<string, string>>): number | undefined {
  return responseError(429, new Headers(headers), body("rate_limit_error", "slow")).retryAfter;
}

function rejected(message: string): boolean {
  return isOpaqueRejection(
    400,
    responseError(400, new Headers(), body("invalid_request_error", message)),
  );
}

test("reads retry delays in every form", () => {
  expect(after({ "retry-after-ms": "1500", "retry-after": "9" })).toBe(1500);
  expect(after({ "retry-after": "12" })).toBe(12_000);
  expect(after({ "retry-after": new Date(Date.now() + 60_000).toUTCString() })).toBeGreaterThan(
    55_000,
  );
  expect(after({})).toBeUndefined();
});

test("keeps the parsed body and survives bodies that are not JSON", () => {
  const parsed = responseError(400, new Headers(), body("invalid_request_error", "bad"));
  expect(parsed).toEqual({
    kind: "invalid",
    message: "bad",
    detail: { type: "error", error: { type: "invalid_request_error", message: "bad" } },
  });
  expect(responseError(502, new Headers(), "<html>Bad gateway</html>")).toEqual({
    kind: "unavailable",
    message: "Anthropic responded with HTTP 502.",
  });
});

test("maps stream error events by type", () => {
  expect(eventError("overloaded_error", "Overloaded").kind).toBe("unavailable");
  expect(eventError("rate_limit_error", "slow").kind).toBe("rate_limit");
  expect(eventError("invalid_request_error", "prompt is too long").kind).toBe("overflow");
  expect(eventError("something_new", "?").kind).toBe("unknown");
});

test("recognizes rejected replay data", () => {
  expect(rejected("messages.1.content.0: Invalid `signature` in `thinking` block")).toBe(true);
  expect(
    rejected(
      "`thinking` or `redacted_thinking` blocks in the latest assistant message cannot be modified.",
    ),
  ).toBe(true);
  expect(rejected("The block is bound to a different conversation.")).toBe(true);
  expect(rejected("prompt is too long")).toBe(false);
});

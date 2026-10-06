import { expect, test } from "bun:test";
import { responseError, responseFailure } from "./error.ts";

function body(code: string | null, message: string, type = "invalid_request_error"): string {
  return JSON.stringify({ error: { message, type, param: null, code } });
}

test("reads auth failures even when they arrive as plain text", () => {
  const error = responseError(
    401,
    new Headers({ "content-type": "text/plain" }),
    body("invalid_api_key", "Incorrect API key provided."),
  );
  expect(error).toMatchObject({ kind: "auth", message: "Incorrect API key provided." });
});

test("tells quota exhaustion apart from rate limits", () => {
  expect(
    responseError(
      429,
      new Headers(),
      body("insufficient_quota", "No credit.", "insufficient_quota"),
    ).kind,
  ).toBe("quota");
  expect(
    responseError(429, new Headers(), body("subscription_sharing_usage_limit_exceeded", "Limit."))
      .kind,
  ).toBe("quota");
  const limited = responseError(
    429,
    new Headers(),
    body("rate_limit_exceeded", "Please try again in 1.5s."),
  );
  expect(limited).toMatchObject({ kind: "rate_limit", retryAfter: 1500 });
});

test("prefers explicit retry headers and falls back to reset headers", () => {
  const millis = responseError(
    429,
    new Headers({ "retry-after-ms": "250", "retry-after": "9" }),
    "",
  );
  expect(millis).toMatchObject({ kind: "rate_limit", retryAfter: 250 });
  const reset = responseError(429, new Headers({ "x-ratelimit-reset-requests": "6m0s" }), "");
  expect(reset.retryAfter).toBe(360_000);
  const short = responseError(429, new Headers({ "x-ratelimit-reset-tokens": "20ms" }), "");
  expect(short.retryAfter).toBe(20);
});

test("maps overflow, refusals, unavailability and plan rejections", () => {
  expect(responseError(400, new Headers(), body("context_length_exceeded", "Too long.")).kind).toBe(
    "overflow",
  );
  expect(responseError(413, new Headers(), "").kind).toBe("overflow");
  expect(responseError(400, new Headers(), body("cyber_policy", "Blocked.")).kind).toBe("refused");
  expect(responseError(503, new Headers(), body("server_is_overloaded", "Busy.")).kind).toBe(
    "unavailable",
  );
  expect(
    responseError(400, new Headers(), body("subscription_sharing_unsupported_capability", "No."))
      .kind,
  ).toBe("unsupported");
  expect(
    responseError(403, new Headers(), body("subscription_sharing_user_not_eligible", "No.")).kind,
  ).toBe("auth");
});

test("keeps the code and parameter that decide a retry", () => {
  const failure = responseFailure(
    400,
    new Headers(),
    JSON.stringify({
      error: {
        message: "Verify.",
        type: "invalid_request_error",
        param: "reasoning.summary",
        code: "unsupported_value",
      },
    }),
  );
  expect(failure).toMatchObject({ code: "unsupported_value", param: "reasoning.summary" });
});

test("reads detail-only bodies and falls back to the status", () => {
  expect(
    responseError(400, new Headers(), JSON.stringify({ detail: "Unsupported parameter: x" })),
  ).toMatchObject({
    kind: "invalid",
    message: "Unsupported parameter: x",
  });
  expect(responseError(502, new Headers(), "<html>")).toEqual({
    kind: "unavailable",
    message: "OpenAI responded with HTTP 502.",
  });
});

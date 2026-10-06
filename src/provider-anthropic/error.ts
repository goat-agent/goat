import type { ErrorKind, Json, ProviderError } from "../provider/index.ts";
import { z } from "zod";
import { errorBody } from "./wire.ts";

interface Body {
  readonly message: string;
  readonly code?: string;
  readonly json: Json;
}

const digits = /^\d+$/u;
const seconds = /^\d+(?:\.\d+)?$/u;
const overflow = /prompt is too long/iu;
const quota = /credit balance|usage limits|extra usage/iu;
const opaqueRejection =
  /Invalid `signature`|Invalid `data` in `redacted_thinking`|cannot be modified|bound to a different conversation/u;

const typeKinds: Readonly<Record<string, ErrorKind>> = {
  api_error: "unavailable",
  overloaded_error: "unavailable",
  timeout_error: "unavailable",
  rate_limit_error: "rate_limit",
  authentication_error: "auth",
  permission_error: "auth",
  billing_error: "quota",
  request_too_large: "overflow",
};

export function responseError(status: number, headers: Headers, text: string): ProviderError {
  const body = bodyOf(text);
  const kind = statusKind(status, headers, body);
  const after =
    kind === "quota" ? (resetAfter(headers) ?? retryAfter(headers)) : retryAfter(headers);
  return {
    kind,
    message: body?.message ?? `Anthropic responded with HTTP ${String(status)}.`,
    ...(after === undefined ? {} : { retryAfter: after }),
    ...(body === undefined ? {} : { detail: body.json }),
  };
}

export function eventError(type: string, message: string): ProviderError {
  return {
    kind: type === "invalid_request_error" ? messageKind(message) : (typeKinds[type] ?? "unknown"),
    message,
  };
}

export function networkError(cause: unknown): ProviderError {
  return { kind: "network", message: cause instanceof Error ? cause.message : String(cause) };
}

export function malformed(message: string): ProviderError {
  return { kind: "malformed", message };
}

export function isOpaqueRejection(status: number, error: ProviderError): boolean {
  return status === 400 && opaqueRejection.test(error.message);
}

function statusKind(status: number, headers: Headers, body: Body | undefined): ErrorKind {
  if (status >= 500 || status === 408 || status === 409) {
    return "unavailable";
  }
  if (status === 401 || status === 403) {
    return "auth";
  }
  if (status === 402) {
    return "quota";
  }
  if (status === 413) {
    return "overflow";
  }
  if (status === 429) {
    const exhausted =
      headers.get("anthropic-ratelimit-unified-status") === "rejected" ||
      body?.code === "enforced_spend_limit_reached";
    return exhausted ? "quota" : "rate_limit";
  }
  return messageKind(body?.message ?? "");
}

function messageKind(message: string): ErrorKind {
  if (overflow.test(message)) {
    return "overflow";
  }
  return quota.test(message) ? "quota" : "invalid";
}

function retryAfter(headers: Headers): number | undefined {
  const milliseconds = headers.get("retry-after-ms");
  if (milliseconds !== null && digits.test(milliseconds)) {
    return Number(milliseconds);
  }
  const value = headers.get("retry-after");
  if (value === null) {
    return undefined;
  }
  if (seconds.test(value)) {
    return Number(value) * 1000;
  }
  const date = Date.parse(value);
  return Number.isNaN(date) ? undefined : Math.max(0, date - Date.now());
}

function resetAfter(headers: Headers): number | undefined {
  const reset = headers.get("anthropic-ratelimit-unified-reset");
  return reset !== null && digits.test(reset)
    ? Math.max(0, Number(reset) * 1000 - Date.now())
    : undefined;
}

function bodyOf(text: string): Body | undefined {
  try {
    const json = z.json().parse(JSON.parse(text));
    const parsed = errorBody.safeParse(json);
    if (!parsed.success) {
      return undefined;
    }
    const { message, details } = parsed.data.error;
    return {
      message,
      json,
      ...(typeof details?.error_code === "string" ? { code: details.error_code } : {}),
    };
  } catch {
    return undefined;
  }
}

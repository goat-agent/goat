import type { ErrorKind, Json, ProviderError } from "../provider/index.ts";
import { z } from "zod";
import { errorBody, type WireFailure } from "./wire.ts";

export interface Failure {
  readonly error: ProviderError;
  readonly code?: string;
  readonly param?: string;
}

interface Body {
  readonly message: string;
  readonly code?: string;
  readonly type?: string;
  readonly param?: string;
  readonly resetsAt?: number;
  readonly json: Json;
}

const digits = /^\d+$/u;
const seconds = /^\d+(?:\.\d+)?$/u;
const tryAgain = /try again in (\d+(?:\.\d+)?)\s*(ms|s)\b/iu;
const duration = /^(?:(\d+)h)?(?:(\d+)m(?!s))?(?:(\d+(?:\.\d+)?)s)?(?:(\d+)ms)?$/u;
const policy = /policy/u;

const codeKinds: Readonly<Record<string, ErrorKind>> = {
  context_length_exceeded: "overflow",
  rate_limit_exceeded: "rate_limit",
  slow_down: "rate_limit",
  insufficient_quota: "quota",
  credit_balance_exhausted: "quota",
  organization_spend_limit_exceeded: "quota",
  project_spend_limit_exceeded: "quota",
  organization_usage_limit_exceeded: "quota",
  subscription_sharing_usage_limit_exceeded: "quota",
  invalid_prompt: "refused",
  server_error: "unavailable",
  server_is_overloaded: "unavailable",
  model_at_capacity: "unavailable",
  subscription_sharing_usage_unavailable: "unavailable",
  subscription_sharing_user_unavailable: "unavailable",
  subscription_sharing_unsupported_capability: "unsupported",
  invalid_api_key: "auth",
  subscription_sharing_invalid_user: "auth",
  subscription_sharing_user_not_eligible: "auth",
};

const typeKinds: Readonly<Record<string, ErrorKind>> = {
  insufficient_quota: "quota",
  rate_limit_error: "rate_limit",
  service_unavailable_error: "unavailable",
  server_error: "unavailable",
  authentication_error: "auth",
};

export function responseFailure(status: number, headers: Headers, text: string): Failure {
  const body = bodyOf(text);
  const kind = statusKind(status, body);
  const after = kind === "quota" ? resetAfter(body) : retryAfter(headers, body?.message);
  return {
    error: {
      kind,
      message: body?.message ?? `OpenAI responded with HTTP ${String(status)}.`,
      ...(after === undefined ? {} : { retryAfter: after }),
      ...(body === undefined ? {} : { detail: body.json }),
    },
    ...(body?.code === undefined ? {} : { code: body.code }),
    ...(body?.param === undefined ? {} : { param: body.param }),
  };
}

export function responseError(status: number, headers: Headers, text: string): ProviderError {
  return responseFailure(status, headers, text).error;
}

export function eventFailure(failure: WireFailure | null | undefined): Failure {
  const code = failure?.code ?? undefined;
  const message = failure?.message ?? "OpenAI reported a failure without a message.";
  const kind = kindOf(code, failure?.type ?? undefined) ?? "unknown";
  const after = kind === "rate_limit" ? messageDelay(message) : undefined;
  return {
    error: { kind, message, ...(after === undefined ? {} : { retryAfter: after }) },
    ...(code === undefined ? {} : { code }),
    ...(typeof failure?.param === "string" ? { param: failure.param } : {}),
  };
}

export function networkError(cause: unknown): ProviderError {
  return { kind: "network", message: cause instanceof Error ? cause.message : String(cause) };
}

export function malformed(message: string): ProviderError {
  return { kind: "malformed", message };
}

export function isOpaqueRejection(failure: Failure): boolean {
  return failure.code === "invalid_encrypted_content";
}

export function isSummaryRejection(failure: Failure): boolean {
  return failure.param === "reasoning.summary";
}

function statusKind(status: number, body: Body | undefined): ErrorKind {
  const known = kindOf(body?.code, body?.type);
  if (known !== undefined) {
    return known;
  }
  if (status >= 500 || status === 408 || status === 409) {
    return "unavailable";
  }
  if (status === 401 || status === 403) {
    return "auth";
  }
  if (status === 413) {
    return "overflow";
  }
  return status === 429 ? "rate_limit" : "invalid";
}

function kindOf(code: string | undefined, type: string | undefined): ErrorKind | undefined {
  if (code !== undefined) {
    const kind = codeKinds[code] ?? (policy.test(code) ? "refused" : undefined);
    if (kind !== undefined) {
      return kind;
    }
  }
  return type === undefined ? undefined : typeKinds[type];
}

function retryAfter(headers: Headers, message: string | undefined): number | undefined {
  const milliseconds = headers.get("retry-after-ms");
  if (milliseconds !== null && digits.test(milliseconds)) {
    return Number(milliseconds);
  }
  const value = headers.get("retry-after");
  if (value !== null && seconds.test(value)) {
    return Number(value) * 1000;
  }
  const date = value === null ? Number.NaN : Date.parse(value);
  if (!Number.isNaN(date)) {
    return Math.max(0, date - Date.now());
  }
  return (
    messageDelay(message) ??
    resetDelay(headers.get("x-ratelimit-reset-requests")) ??
    resetDelay(headers.get("x-ratelimit-reset-tokens"))
  );
}

function messageDelay(message: string | undefined): number | undefined {
  const match = tryAgain.exec(message ?? "");
  if (match?.[1] === undefined) {
    return undefined;
  }
  const value = Number(match[1]);
  return Math.ceil(match[2]?.toLowerCase() === "ms" ? value : value * 1000);
}

function resetDelay(value: string | null): number | undefined {
  const match = value === null ? null : duration.exec(value);
  if (match === null || value === "") {
    return undefined;
  }
  const [, hours, minutes, wholeSeconds, milliseconds] = match;
  return Math.ceil(
    Number(hours ?? 0) * 3_600_000 +
      Number(minutes ?? 0) * 60_000 +
      Number(wholeSeconds ?? 0) * 1000 +
      Number(milliseconds ?? 0),
  );
}

function resetAfter(body: Body | undefined): number | undefined {
  return body?.resetsAt === undefined ? undefined : Math.max(0, body.resetsAt * 1000 - Date.now());
}

function bodyOf(text: string): Body | undefined {
  try {
    const json = z.json().parse(JSON.parse(text));
    const parsed = errorBody.safeParse(json);
    if (!parsed.success) {
      return undefined;
    }
    if ("detail" in parsed.data) {
      return { message: parsed.data.detail, json };
    }
    const { message, code, type, param, resets_at: resetsAt } = parsed.data.error;
    return {
      message: message ?? "OpenAI reported an error without a message.",
      json,
      ...(typeof code === "string" ? { code } : {}),
      ...(typeof type === "string" ? { type } : {}),
      ...(typeof param === "string" ? { param } : {}),
      ...(typeof resetsAt === "number" ? { resetsAt } : {}),
    };
  } catch {
    return undefined;
  }
}

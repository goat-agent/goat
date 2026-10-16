import type { Json } from "@goat/json";

export type ErrorKind =
  | "auth"
  | "rate_limit"
  | "quota"
  | "overflow"
  | "invalid"
  | "unsupported"
  | "refused"
  | "unavailable"
  | "network"
  | "malformed"
  | "unknown";

export interface ProviderError {
  readonly kind: ErrorKind;
  readonly message: string;
  readonly retryAfter?: number;
  readonly detail?: Json;
}

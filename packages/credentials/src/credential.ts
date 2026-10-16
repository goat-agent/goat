import type { Json } from "@goat/json";
import type { Result } from "@goat/result";

export type Fields = { readonly [field: string]: Json };

export interface Credential {
  readonly provider: string;
  readonly method: string;
  readonly fields: Fields;
}

export interface Method<T> {
  readonly provider: string;
  readonly method: string;
  connect(fields: Fields): Result<T, string>;
}

export interface CredentialError {
  readonly kind: "invalid" | "missing" | "malformed" | "unsupported" | "unavailable";
  readonly message: string;
}

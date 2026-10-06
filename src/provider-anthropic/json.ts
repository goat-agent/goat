import type { Json } from "../provider/index.ts";

export type JsonObject = { readonly [key: string]: Json };

export type Frozen<T> = T extends object ? { readonly [K in keyof T]: Frozen<T[K]> } : T;

export function asObject(value: Json | undefined): JsonObject | undefined {
  if (typeof value !== "object" || value === null || isArray(value)) {
    return undefined;
  }
  return value;
}

export function withoutKeys(object: JsonObject, keys: readonly string[]): JsonObject {
  return Object.fromEntries(
    Object.entries(object).filter(
      ([key, value]: readonly [string, Json]) => !keys.includes(key) && value !== null,
    ),
  );
}

function isArray(value: Json): value is readonly Json[] {
  return Array.isArray(value);
}

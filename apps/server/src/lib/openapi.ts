import { z } from "@hono/zod-openapi";
import type { Context, Env } from "hono";

export const ErrorBody = z.object({ error: z.string() }).openapi("Error");

export const document = {
  openapi: "3.1.0",
  info: { title: "Goat API", version: "0.1.0" },
};

export function json<T extends z.ZodType>(schema: T, description: string) {
  return { content: { "application/json": { schema } }, description };
}

export function body<T extends z.ZodType>(schema: T, description: string) {
  return { ...json(schema, description), required: true };
}

export function error(description: string) {
  return json(ErrorBody, description);
}

// Builds already in users' hands must still decode values added later, so enum fields also accept any string.
export function openEnum<const T extends readonly [string, ...string[]]>(values: T) {
  return z.union([z.enum(values), z.string()]);
}

export function invalidRequest<E extends Env>(result: { success: boolean }, c: Context<E>) {
  return result.success ? undefined : c.json({ error: "invalid request" }, 400);
}

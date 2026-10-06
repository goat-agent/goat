import { z } from "zod";
import type { Frozen } from "./json.ts";

const json = z.json();
const index = z.number().int().nonnegative();
const count = z.number().nullish();
const text = z.string().nullish();

const object = z.record(z.string(), json);

const usage = z.object({
  input_tokens: count,
  input_tokens_details: z.object({ cached_tokens: count, cache_write_tokens: count }).nullish(),
  output_tokens: count,
  output_tokens_details: z.object({ reasoning_tokens: count }).nullish(),
});

const failure = z.object({ code: text, message: text, param: text, type: text });

export const terminal = z.object({
  response: z.object({
    usage: usage.nullish(),
    service_tier: text,
    incomplete_details: z.object({ reason: text }).nullish(),
    error: failure.nullish(),
  }),
});

export const errorEvent = z.object({ error: failure.nullish() }).and(failure);

export const errorBody = z.union([
  z.object({ error: failure.extend({ resets_at: count }) }),
  z.object({ detail: z.string() }),
]);

export const item = z.object({ output_index: index, item: object });

export const summaryPart = z.object({
  output_index: index,
  summary_index: index,
  part: z.object({ text: z.string() }).nullish(),
});

export const contentPart = z.object({
  output_index: index,
  content_index: index,
  part: z.object({ type: z.string() }),
});

export const outputDelta = z.object({ output_index: index, delta: z.string() });

export const contentDelta = outputDelta.extend({ content_index: index });

export const summaryDelta = outputDelta.extend({ summary_index: index });

export const reasoningItem = z.object({
  id: z.string(),
  encrypted_content: text,
  summary: z.array(z.object({ text: z.string() })).nullish(),
});

export const messageItem = z.object({
  id: z.string(),
  content: z.array(object),
});

export const callItem = z.object({
  call_id: z.string(),
  name: z.string(),
  arguments: z.string(),
});

export const modelList = z.object({ data: z.array(z.object({ id: z.string() })) });

export const planModelList = z.object({
  models: z.array(z.object({ slug: z.string(), visibility: text })),
});

export type WireUsage = Frozen<z.output<typeof usage>>;
export type WireResponse = Frozen<z.output<typeof terminal>["response"]>;
export type WireFailure = Frozen<z.output<typeof failure>>;

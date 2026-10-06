import { z } from "zod";
import type { Frozen } from "./json.ts";

const json = z.json();
const index = z.number().int().nonnegative();
const count = z.number().nullish();
const flag = z.object({ supported: z.boolean() }).nullish();

export const block = z.record(z.string(), json);

const usage = z.object({
  input_tokens: count,
  output_tokens: count,
  cache_creation_input_tokens: count,
  cache_read_input_tokens: count,
  cache_creation: z.object({ ephemeral_1h_input_tokens: count }).nullish(),
  output_tokens_details: z.object({ thinking_tokens: count }).nullish(),
});

export const messageStart = z.object({ message: z.object({ usage: usage.nullish() }) });

export const blockStart = z.object({ index, content_block: block });

export const blockDelta = z.object({ index, delta: z.looseObject({ type: z.string() }) });

export const delta = z.discriminatedUnion("type", [
  z.object({ type: z.literal("text_delta"), text: z.string() }),
  z.object({ type: z.literal("thinking_delta"), thinking: z.string() }),
  z.object({ type: z.literal("input_json_delta"), partial_json: z.string() }),
  z.object({ type: z.literal("signature_delta"), signature: z.string() }),
  z.object({ type: z.literal("citations_delta"), citation: json }),
]);

export const blockStop = z.object({ index });

export const messageDelta = z.object({
  delta: z.object({ stop_reason: z.string().nullish() }),
  usage: usage.nullish(),
});

export const errorEvent = z.object({ error: z.object({ type: z.string(), message: z.string() }) });

export const errorBody = z.object({
  error: z.object({
    type: z.string(),
    message: z.string(),
    details: z.object({ error_code: z.string().nullish() }).nullish(),
  }),
});

export const liveModel = z.object({
  id: z.string(),
  display_name: z.string(),
  max_input_tokens: z.number(),
  max_tokens: z.number(),
  capabilities: z
    .object({
      effort: z.object({ low: flag, medium: flag, high: flag, xhigh: flag, max: flag }).nullish(),
      image_input: flag,
      pdf_input: flag,
      structured_outputs: flag,
      thinking: z
        .object({ types: z.object({ adaptive: flag, enabled: flag, disabled: flag }) })
        .nullish(),
    })
    .nullish(),
});

export const modelList = z.object({ data: z.array(z.unknown()) });

export type WireUsage = Frozen<z.output<typeof usage>>;
export type LiveModel = Frozen<z.output<typeof liveModel>>;

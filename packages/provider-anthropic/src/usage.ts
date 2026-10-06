import type { Usage } from "@goat/provider";
import type { Price } from "./catalog.ts";
import type { WireUsage } from "./wire.ts";

export interface Counts {
  readonly input: number | undefined;
  readonly output: number | undefined;
  readonly cacheRead: number | undefined;
  readonly cacheWrite: number | undefined;
  readonly cacheWriteLong: number | undefined;
  readonly reasoning: number | undefined;
}

export const noCounts: Counts = {
  input: undefined,
  output: undefined,
  cacheRead: undefined,
  cacheWrite: undefined,
  cacheWriteLong: undefined,
  reasoning: undefined,
};

export function addCounts(counts: Counts, wire: WireUsage): Counts {
  return {
    input: wire.input_tokens ?? counts.input,
    output: wire.output_tokens ?? counts.output,
    cacheRead: wire.cache_read_input_tokens ?? counts.cacheRead,
    cacheWrite: wire.cache_creation_input_tokens ?? counts.cacheWrite,
    cacheWriteLong: wire.cache_creation?.ephemeral_1h_input_tokens ?? counts.cacheWriteLong,
    reasoning: wire.output_tokens_details?.thinking_tokens ?? counts.reasoning,
  };
}

export function usageOf(counts: Counts, price: Price | undefined): Usage | undefined {
  if (counts.input === undefined && counts.output === undefined) {
    return undefined;
  }
  const input = counts.input ?? 0;
  const output = counts.output ?? 0;
  const read = counts.cacheRead ?? 0;
  const write = counts.cacheWrite ?? 0;
  const long = Math.min(counts.cacheWriteLong ?? 0, write);
  return {
    input: {
      total: input + read + write,
      ...(counts.cacheRead === undefined ? {} : { cacheRead: counts.cacheRead }),
      ...(counts.cacheWrite === undefined ? {} : { cacheWrite: counts.cacheWrite }),
    },
    output: {
      total: output,
      ...(counts.reasoning === undefined ? {} : { reasoning: counts.reasoning }),
    },
    ...(price === undefined
      ? {}
      : {
          cost:
            (input * price.input +
              read * price.cacheRead +
              (write - long) * price.cacheWrite +
              long * price.cacheWriteLong +
              output * price.output) /
            1_000_000,
        }),
  };
}

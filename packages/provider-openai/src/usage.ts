import type { Usage } from "../provider/index.ts";
import { longContextThreshold, type Profile } from "./catalog.ts";
import type { WireUsage } from "./wire.ts";

export function usageOf(
  wire: WireUsage | null | undefined,
  tier: string | undefined,
  profile: Profile | undefined,
): Usage | undefined {
  const input = wire?.input_tokens ?? undefined;
  const output = wire?.output_tokens ?? undefined;
  if (input === undefined && output === undefined) {
    return undefined;
  }
  const cacheRead = wire?.input_tokens_details?.cached_tokens ?? undefined;
  const cacheWrite = wire?.input_tokens_details?.cache_write_tokens ?? undefined;
  const reasoning = wire?.output_tokens_details?.reasoning_tokens ?? undefined;
  const counts = {
    input: input ?? 0,
    output: output ?? 0,
    cacheRead: cacheRead ?? 0,
    cacheWrite: cacheWrite ?? 0,
  };
  const cost = costOf(counts, tier, profile);
  return {
    input: {
      total: counts.input,
      ...(cacheRead === undefined ? {} : { cacheRead }),
      ...(cacheWrite === undefined ? {} : { cacheWrite }),
    },
    output: { total: counts.output, ...(reasoning === undefined ? {} : { reasoning }) },
    ...(cost === undefined ? {} : { cost }),
  };
}

function costOf(
  counts: {
    readonly input: number;
    readonly output: number;
    readonly cacheRead: number;
    readonly cacheWrite: number;
  },
  tier: string | undefined,
  profile: Profile | undefined,
): number | undefined {
  const price = profile?.info.price;
  const multiplier = profile?.tiers[tier ?? "default"];
  if (price === undefined || multiplier === undefined) {
    return undefined;
  }
  const long = profile?.longContext === true && counts.input > longContextThreshold;
  const uncached = Math.max(0, counts.input - counts.cacheRead - counts.cacheWrite);
  const input =
    uncached * price.input +
    counts.cacheRead * (price.cacheRead ?? price.input) +
    counts.cacheWrite * (price.cacheWrite ?? price.input);
  const output = counts.output * price.output;
  return ((long ? input * 2 + output * 1.5 : input + output) * multiplier) / 1_000_000;
}

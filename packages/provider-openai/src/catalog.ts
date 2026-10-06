import type { ModelInfo } from "../provider/index.ts";

export interface Profile {
  readonly info: ModelInfo;
  readonly longContext: boolean;
  readonly tiers: Readonly<Record<string, number>>;
}

type Effort = (typeof efforts)[number];

interface Price {
  readonly input: number;
  readonly cacheRead: number;
  readonly cacheWrite?: number;
  readonly output: number;
}

interface Entry {
  readonly id: string;
  readonly aliases?: readonly string[];
  readonly name: string;
  readonly contextWindow: number;
  readonly longContext: boolean;
  readonly efforts: readonly Effort[];
  readonly defaultEffort: Effort;
  readonly price: Price;
  readonly fast: number;
}

const efforts = ["none", "low", "medium", "high", "xhigh", "max"] as const;

export const longContextThreshold = 272_000;

const media = [
  "image/png",
  "image/jpeg",
  "image/webp",
  "image/gif",
  "application/pdf",
  "text/plain",
  "text/markdown",
  "text/csv",
  "text/tab-separated-values",
  "text/html",
  "application/json",
  "application/xml",
  "application/rtf",
  "application/msword",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.oasis.opendocument.text",
  "application/vnd.ms-excel",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  "application/vnd.ms-powerpoint",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation",
];

const large = { contextWindow: 1_050_000, longContext: true };
const thinking = efforts.filter((effort) => effort !== "none");
const upToXhigh = efforts.filter((effort) => effort !== "max");

const entries: readonly Entry[] = [
  {
    id: "gpt-6-astra",
    name: "GPT-6 Astra",
    ...large,
    efforts: thinking,
    defaultEffort: "medium",
    price: { input: 10, cacheRead: 1, cacheWrite: 12.5, output: 50 },
    fast: 2,
  },
  {
    id: "gpt-6.1-sol",
    name: "GPT-6.1 Sol",
    ...large,
    efforts: thinking,
    defaultEffort: "medium",
    price: { input: 2, cacheRead: 0.1, cacheWrite: 2.5, output: 10 },
    fast: 2,
  },
  {
    id: "gpt-6-sol",
    name: "GPT-6 Sol",
    ...large,
    efforts,
    defaultEffort: "medium",
    price: { input: 2, cacheRead: 0.2, cacheWrite: 2.5, output: 10 },
    fast: 2,
  },
  {
    id: "gpt-6-luna",
    name: "GPT-6 Luna",
    ...large,
    efforts,
    defaultEffort: "medium",
    price: { input: 0.1, cacheRead: 0.01, cacheWrite: 0.125, output: 0.5 },
    fast: 2,
  },
  {
    id: "gpt-5.6-sol",
    aliases: ["gpt-5.6"],
    name: "GPT-5.6 Sol",
    ...large,
    efforts,
    defaultEffort: "medium",
    price: { input: 4, cacheRead: 0.4, cacheWrite: 5, output: 20 },
    fast: 2,
  },
  {
    id: "gpt-5.6-terra",
    name: "GPT-5.6 Terra",
    ...large,
    efforts,
    defaultEffort: "medium",
    price: { input: 2, cacheRead: 0.2, cacheWrite: 2.5, output: 12 },
    fast: 2,
  },
  {
    id: "gpt-5.6-luna",
    name: "GPT-5.6 Luna",
    ...large,
    efforts,
    defaultEffort: "medium",
    price: { input: 0.2, cacheRead: 0.02, cacheWrite: 0.25, output: 1.2 },
    fast: 2,
  },
  {
    id: "gpt-5.5",
    aliases: ["gpt-5.5-2026-04-23"],
    name: "GPT-5.5",
    ...large,
    efforts: upToXhigh,
    defaultEffort: "medium",
    price: { input: 5, cacheRead: 0.5, output: 30 },
    fast: 2.5,
  },
  {
    id: "gpt-5.4",
    aliases: ["gpt-5.4-2026-03-05"],
    name: "GPT-5.4",
    ...large,
    efforts: upToXhigh,
    defaultEffort: "none",
    price: { input: 2.5, cacheRead: 0.25, output: 15 },
    fast: 2,
  },
  {
    id: "gpt-5.4-mini",
    aliases: ["gpt-5.4-mini-2026-03-17"],
    name: "GPT-5.4 mini",
    contextWindow: 400_000,
    longContext: false,
    efforts: upToXhigh,
    defaultEffort: "none",
    price: { input: 0.75, cacheRead: 0.075, output: 4.5 },
    fast: 2,
  },
];

export function catalogProfile(id: string, priced: boolean): Profile | undefined {
  const entry = entries.find(
    (candidate) => candidate.id === id || candidate.aliases?.includes(id) === true,
  );
  if (entry === undefined) {
    return undefined;
  }
  return {
    info: {
      id,
      name: entry.name,
      contextWindow: entry.contextWindow,
      maxOutputTokens: 128_000,
      supports: { media, tools: true, outputSchema: true },
      effort: { levels: entry.efforts, default: entry.defaultEffort },
      ...(priced ? { price: entry.price } : {}),
    },
    longContext: entry.longContext,
    tiers: { default: 1, flex: 0.5, priority: entry.fast, fast: entry.fast },
  };
}

export function catalogProfiles(priced: boolean): readonly Profile[] {
  return entries.flatMap((entry) => catalogProfile(entry.id, priced) ?? []);
}

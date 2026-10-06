import type { ModelInfo } from "@goat/provider";
import type { LiveModel } from "./wire.ts";

export interface Price {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
  readonly cacheWriteLong: number;
}

type Effort = (typeof efforts)[number];

type Thinking =
  | { readonly kind: "adaptive"; readonly efforts: readonly Effort[]; readonly canDisable: boolean }
  | { readonly kind: "budget" }
  | { readonly kind: "none" };

export interface Profile {
  readonly info: ModelInfo;
  readonly thinking: Thinking;
  readonly systemMessages: boolean;
  readonly bindsThinking: boolean;
  readonly price?: Price;
}

interface Entry {
  readonly id: string;
  readonly aliases?: readonly string[];
  readonly name: string;
  readonly contextWindow: number;
  readonly maxOutputTokens: number;
  readonly thinking: Thinking;
  readonly defaultEffort: string;
  readonly systemMessages: boolean;
  readonly bindsThinking: boolean;
  readonly price: Price;
}

const efforts = ["low", "medium", "high", "xhigh", "max"] as const;

export const budgets: Readonly<Record<string, number>> = {
  low: 4096,
  medium: 16_384,
  high: 32_768,
};

const images = ["image/jpeg", "image/png", "image/gif", "image/webp"];
const large = { contextWindow: 1_000_000, maxOutputTokens: 128_000 };
const withoutXhigh = efforts.filter((effort) => effort !== "xhigh");

const entries: readonly Entry[] = [
  {
    id: "claude-fable-5-1",
    name: "Claude Fable 5.1",
    ...large,
    thinking: adaptive(efforts, false),
    defaultEffort: "high",
    systemMessages: true,
    bindsThinking: true,
    price: price(10, 50, 0.25),
  },
  {
    id: "claude-fable-5",
    name: "Claude Fable 5",
    ...large,
    thinking: adaptive(efforts, false),
    defaultEffort: "high",
    systemMessages: true,
    bindsThinking: false,
    price: price(10, 50, 1),
  },
  {
    id: "claude-opus-5-5",
    name: "Claude Opus 5.5",
    ...large,
    thinking: adaptive(efforts, false),
    defaultEffort: "medium",
    systemMessages: true,
    bindsThinking: true,
    price: price(4, 20, 0.2),
  },
  {
    id: "claude-opus-5",
    name: "Claude Opus 5",
    ...large,
    thinking: adaptive(efforts, true),
    defaultEffort: "high",
    systemMessages: true,
    bindsThinking: false,
    price: price(5, 25, 0.5),
  },
  {
    id: "claude-opus-4-8",
    name: "Claude Opus 4.8",
    ...large,
    thinking: adaptive(efforts, true),
    defaultEffort: "high",
    systemMessages: true,
    bindsThinking: false,
    price: price(5, 25, 0.5),
  },
  {
    id: "claude-opus-4-7",
    name: "Claude Opus 4.7",
    ...large,
    thinking: adaptive(efforts, true),
    defaultEffort: "high",
    systemMessages: false,
    bindsThinking: false,
    price: price(5, 25, 0.5),
  },
  {
    id: "claude-opus-4-6",
    name: "Claude Opus 4.6",
    ...large,
    thinking: adaptive(withoutXhigh, true),
    defaultEffort: "high",
    systemMessages: false,
    bindsThinking: false,
    price: price(5, 25, 0.5),
  },
  {
    id: "claude-sonnet-5-5",
    name: "Claude Sonnet 5.5",
    ...large,
    thinking: adaptive(efforts, false),
    defaultEffort: "high",
    systemMessages: true,
    bindsThinking: true,
    price: price(2, 10, 0.2),
  },
  {
    id: "claude-sonnet-5",
    name: "Claude Sonnet 5",
    ...large,
    thinking: adaptive(efforts, true),
    defaultEffort: "high",
    systemMessages: false,
    bindsThinking: false,
    price: price(2, 10, 0.2),
  },
  {
    id: "claude-sonnet-4-6",
    name: "Claude Sonnet 4.6",
    ...large,
    thinking: adaptive(withoutXhigh, true),
    defaultEffort: "high",
    systemMessages: false,
    bindsThinking: false,
    price: price(3, 15, 0.3),
  },
  {
    id: "claude-haiku-4-5-20251001",
    aliases: ["claude-haiku-4-5"],
    name: "Claude Haiku 4.5",
    contextWindow: 200_000,
    maxOutputTokens: 64_000,
    thinking: { kind: "budget" },
    defaultEffort: "none",
    systemMessages: false,
    bindsThinking: false,
    price: price(1, 5, 0.1),
  },
];

export function catalogProfile(id: string): Profile | undefined {
  const entry = entries.find(
    (candidate) => candidate.id === id || candidate.aliases?.includes(id) === true,
  );
  if (entry === undefined) {
    return undefined;
  }
  return {
    info: modelInfo(id, entry),
    thinking: entry.thinking,
    systemMessages: entry.systemMessages,
    bindsThinking: entry.bindsThinking,
    price: entry.price,
  };
}

export function catalogProfiles(): readonly Profile[] {
  return entries.flatMap((entry) => catalogProfile(entry.id) ?? []);
}

export function liveProfile(model: LiveModel): Profile {
  const capabilities = model.capabilities;
  const supported = efforts.filter((effort) => capabilities?.effort?.[effort]?.supported === true);
  const thinking = liveThinking(model, supported);
  const levels = effortLevels(thinking);
  return {
    info: {
      id: model.id,
      name: model.display_name,
      contextWindow: model.max_input_tokens,
      maxOutputTokens: model.max_tokens,
      supports: {
        media: [
          ...(capabilities?.image_input?.supported === true ? images : []),
          ...(capabilities?.pdf_input?.supported === true ? ["application/pdf"] : []),
          "text/plain",
        ],
        tools: true,
        outputSchema: capabilities?.structured_outputs?.supported === true,
      },
      ...effortInfo(levels, liveDefault(thinking, levels)),
    },
    thinking,
    systemMessages: false,
    bindsThinking: false,
  };
}

function liveThinking(model: LiveModel, supported: readonly Effort[]): Thinking {
  const types = model.capabilities?.thinking?.types;
  if (types?.adaptive?.supported === true) {
    return adaptive(supported, types.disabled?.supported === true);
  }
  return types?.enabled?.supported === true ? { kind: "budget" } : { kind: "none" };
}

function liveDefault(thinking: Thinking, levels: readonly string[]): string {
  if (thinking.kind === "budget") {
    return "none";
  }
  return levels.includes("high") ? "high" : (levels.at(-1) ?? "");
}

function modelInfo(id: string, entry: Entry): ModelInfo {
  const { input, output, cacheRead, cacheWrite } = entry.price;
  return {
    id,
    name: entry.name,
    contextWindow: entry.contextWindow,
    maxOutputTokens: entry.maxOutputTokens,
    supports: {
      media: [...images, "application/pdf", "text/plain"],
      tools: true,
      outputSchema: true,
    },
    ...effortInfo(effortLevels(entry.thinking), entry.defaultEffort),
    price: { input, output, cacheRead, cacheWrite },
  };
}

function effortInfo(levels: readonly string[], defaultEffort: string): Pick<ModelInfo, "effort"> {
  return levels.length === 0 ? {} : { effort: { levels, default: defaultEffort } };
}

function effortLevels(thinking: Thinking): readonly string[] {
  if (thinking.kind === "adaptive") {
    return thinking.canDisable ? ["none", ...thinking.efforts] : thinking.efforts;
  }
  return thinking.kind === "budget" ? ["none", ...Object.keys(budgets)] : [];
}

function adaptive(levels: readonly Effort[], canDisable: boolean): Thinking {
  return { kind: "adaptive", efforts: levels, canDisable };
}

function price(input: number, output: number, cacheRead: number): Price {
  return { input, output, cacheRead, cacheWrite: input * 1.25, cacheWriteLong: input * 2 };
}

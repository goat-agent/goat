import { expect, test } from "bun:test";
import { catalogProfile, liveProfile } from "./catalog.ts";

test("describes catalog models under the id they were asked for", () => {
  const haiku = catalogProfile("claude-haiku-4-5");
  expect(haiku?.info.id).toBe("claude-haiku-4-5");
  expect(haiku?.info.effort).toEqual({
    levels: ["none", "low", "medium", "high"],
    default: "none",
  });
  expect(haiku?.thinking).toEqual({ kind: "budget" });
  expect(catalogProfile("claude-haiku-4-5-20251001")?.info.contextWindow).toBe(200_000);
  expect(catalogProfile("claude-unknown")).toBeUndefined();
});

test("offers no way to switch thinking off where the model always thinks", () => {
  const opus = catalogProfile("claude-opus-5-5");
  expect(opus?.info.effort).toEqual({
    levels: ["low", "medium", "high", "xhigh", "max"],
    default: "medium",
  });
  expect(opus?.info.price).toEqual({ input: 4, output: 20, cacheRead: 0.2, cacheWrite: 5 });
  expect(catalogProfile("claude-sonnet-4-6")?.info.effort?.levels).toEqual([
    "none",
    "low",
    "medium",
    "high",
    "max",
  ]);
});

test("describes unknown models from their capabilities", () => {
  const flag = { supported: true };
  const adaptive = liveProfile({
    id: "claude-future-6",
    display_name: "Claude Future 6",
    max_input_tokens: 2_000_000,
    max_tokens: 256_000,
    capabilities: {
      effort: { low: flag, medium: flag, high: flag, xhigh: { supported: false }, max: flag },
      image_input: flag,
      pdf_input: { supported: false },
      structured_outputs: flag,
      thinking: { types: { adaptive: flag, enabled: { supported: false }, disabled: flag } },
    },
  });
  expect(adaptive.info).toEqual({
    id: "claude-future-6",
    name: "Claude Future 6",
    contextWindow: 2_000_000,
    maxOutputTokens: 256_000,
    supports: {
      media: ["image/jpeg", "image/png", "image/gif", "image/webp", "text/plain"],
      tools: true,
      outputSchema: true,
    },
    effort: { levels: ["none", "low", "medium", "high", "max"], default: "high" },
  });
  expect(adaptive.price).toBeUndefined();
  const budget = liveProfile({
    id: "claude-old",
    display_name: "Claude Old",
    max_input_tokens: 200_000,
    max_tokens: 64_000,
    capabilities: { thinking: { types: { enabled: flag } } },
  });
  expect(budget.thinking).toEqual({ kind: "budget" });
  expect(budget.info.effort).toEqual({
    levels: ["none", "low", "medium", "high"],
    default: "none",
  });
});

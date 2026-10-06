import { expect, test } from "bun:test";
import { catalogProfile, catalogProfiles } from "./catalog.ts";

test("describes catalog models under the id they were asked for", () => {
  expect(catalogProfile("gpt-5.6", true)?.info).toMatchObject({
    id: "gpt-5.6",
    name: "GPT-5.6 Sol",
  });
  expect(catalogProfile("gpt-5.5-2026-04-23", true)?.info.contextWindow).toBe(1_050_000);
  expect(catalogProfile("gpt-unknown", true)).toBeUndefined();
});

test("offers no way to switch reasoning off where the model always reasons", () => {
  expect(catalogProfile("gpt-6-astra", true)?.info.effort?.levels).toEqual([
    "low",
    "medium",
    "high",
    "xhigh",
    "max",
  ]);
  expect(catalogProfile("gpt-6-luna", true)?.info.effort?.levels[0]).toBe("none");
  expect(catalogProfile("gpt-5.4", true)?.info.effort).toEqual({
    levels: ["none", "low", "medium", "high", "xhigh"],
    default: "none",
  });
});

test("prices only accounts that are billed per token", () => {
  expect(catalogProfile("gpt-6.1-sol", true)?.info.price).toEqual({
    input: 2,
    cacheRead: 0.1,
    cacheWrite: 2.5,
    output: 10,
  });
  expect(catalogProfile("gpt-6.1-sol", false)?.info.price).toBeUndefined();
  expect(catalogProfiles(false).every((profile) => profile.info.price === undefined)).toBe(true);
  expect(catalogProfile("gpt-5.4-mini", true)?.longContext).toBe(false);
});

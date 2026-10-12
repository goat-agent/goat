import { afterEach, expect, spyOn, test } from "bun:test";
import { methods } from "./methods.ts";

afterEach(() => {
  spyOn(globalThis, "fetch").mockRestore();
});

test("stores API keys as openai apiKey", () => {
  expect(methods.map((method) => [method.provider, method.method])).toEqual([["openai", "apiKey"]]);
});

test("sends requests to the stored base URL", async () => {
  const fetch = spyOn(globalThis, "fetch").mockResolvedValue(new Response("{}", { status: 500 }));
  const connected = methods[0]?.connect({ key: "sk", baseUrl: "https://gateway.example" });
  expect(connected?.ok === true ? connected.value.id : undefined).toBe("openai");
  await (connected?.ok === true ? connected.value.models() : undefined);
  const [url] = fetch.mock.calls[0] ?? [];
  expect(typeof url === "string" ? url : "").toStartWith("https://gateway.example/models");
});

test("accepts https base URLs, and http only for localhost", () => {
  const results = [
    "https://gateway.example",
    "http://localhost:4000",
    "http://127.0.0.1:11434/v1",
    "http://[::1]:8080",
  ].map((baseUrl) => methods[0]?.connect({ key: "sk", baseUrl }));
  expect(results.every((result) => result?.ok === true)).toBe(true);
});

test("refuses keys that cannot be sent and base URLs that would expose them", () => {
  const refused = (
    [
      { key: "" },
      { key: "sk with space" },
      { key: "sk\nInjected: 1" },
      { key: "sk", baseUrl: "http://gateway.example" },
      { key: "sk", baseUrl: "file:///etc/passwd" },
      { key: "sk", baseUrl: "http://localhost.gateway.example" },
      { key: "sk", baseUrl: "not a url" },
    ] as const
  ).map((fields) => methods[0]?.connect(fields));
  expect(refused.every((result) => result?.ok === false)).toBe(true);
});

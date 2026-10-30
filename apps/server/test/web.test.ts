import { beforeEach, describe, expect, test } from "bun:test";
import * as web from "../src/lib/web";
import { llm, webBackend } from "./fakes";
import { createAgent, listMessages, resetDatabase, send, signIn, waitForIdle } from "./helpers";

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
  webBackend.reset();
});

describe("web.fetch", () => {
  test("returns a long page in parts", async () => {
    webBackend.page = { title: "Long", url: "", markdown: "a".repeat(45_000) };

    const first = await web.fetch("https://example.com");
    expect(first).toMatchObject({ truncated: true, nextOffset: 20_000 });
    expect(first.content).toHaveLength(20_000);

    const last = await web.fetch("https://example.com", 40_000);
    expect(last.truncated).toBe(false);
    expect(last.content).toHaveLength(5_000);
    expect(last).not.toHaveProperty("nextOffset");
  });
});

describe("web tools", () => {
  test("search results reach the model wrapped as untrusted, and only the answer is shown", async () => {
    const token = await signIn("minho@example.com", "민호");
    const agentId = await createAgent(token);
    webBackend.results = [
      { title: "Busan weather", url: "https://weather.example/busan", snippet: "Sunny, 18°C" },
    ];
    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 1
          ? { toolName: "web_search", input: { query: "부산 날씨" } }
          : "내일 부산은 맑아요.",
      );

    await send(token, agentId, "내일 부산 날씨 어때?");
    await waitForIdle(agentId);

    expect(webBackend.queries).toEqual(["부산 날씨"]);
    const secondPrompt = JSON.stringify(llm.prompts[1]);
    expect(secondPrompt).toContain("Sunny, 18°C");
    expect(secondPrompt).toMatch(/<web-content id=\\"\w+\\">/);

    const shown = (await listMessages(token, agentId)).map(({ role, text }) => `${role}: ${text}`);
    expect(shown.slice(1)).toEqual([
      "user: 내일 부산 날씨 어때?",
      "assistant: 내일 부산은 맑아요.",
    ]);
  });
});

import { beforeEach, describe, expect, test } from "bun:test";
import { generateText, isStepCount, tool, type ToolSet } from "ai";
import { z } from "zod";
import { searchTools } from "../src/lib/ai/tools/search";
import { fakeModel, llm } from "./fakes";
import { bearer, client, createAgent, resetDatabase, send, signIn, waitForIdle } from "./helpers";

const sent: string[] = [];
const reachable: ToolSet = {
  slack_send_message: tool({
    description: "Send a Slack message to a channel or person.",
    inputSchema: z.object({ channel: z.string(), text: z.string() }),
    execute: ({ channel, text }) => {
      sent.push(`${channel}: ${text}`);
      return { sent: true };
    },
  }),
  slack_search: tool({
    description: "Search Slack messages.",
    inputSchema: z.object({ query: z.string() }),
  }),
  calendar_create_event: tool({
    description: "Create an event on the user's calendar.",
    inputSchema: z.object({ title: z.string(), start: z.string().describe("Start date and time") }),
  }),
};

type Call = { toolName: string; input: object };

const catalog = {
  search: () => Promise.resolve(reachable),
  resolve: (name: string) => {
    const target = reachable[name];
    if (!target) throw new Error(`No tool named ${name}; find it with tool_search.`);
    return Promise.resolve(target);
  },
};

async function resultOf(call: Call) {
  llm.reset();
  llm.reply = () => Promise.resolve(llm.prompts.length === 1 ? call : "done");
  await generateText({
    model: fakeModel,
    prompt: "hi",
    tools: searchTools,
    toolsContext: { tool_search: catalog, tool_call: catalog },
    stopWhen: isStepCount(2),
  });
  const message = llm.prompts[1]?.at(-1);
  const part = message?.role === "tool" ? message.content[0] : undefined;
  return part?.type === "tool-result" ? part.output : undefined;
}

async function errorOf(call: Call) {
  const output = await resultOf(call);
  return output?.type === "error-text" ? output.value : undefined;
}

beforeEach(() => {
  sent.length = 0;
});

describe("tool_search", () => {
  test("ranks tools by what they do and returns their input schemas", async () => {
    expect(
      await resultOf({ toolName: "tool_search", input: { query: "send slack message" } }),
    ).toMatchObject({
      type: "json",
      value: {
        tools: [
          { name: "slack_send_message", inputSchema: { required: ["channel", "text"] } },
          { name: "slack_search" },
        ],
      },
    });
  });

  test("finds a tool by its exact name", async () => {
    expect(
      await resultOf({ toolName: "tool_search", input: { query: "calendar_create_event" } }),
    ).toMatchObject({ value: { tools: [{ name: "calendar_create_event" }] } });
  });
});

describe("tool_call", () => {
  test("runs the named tool with checked input", async () => {
    const input = { channel: "general", text: "10분 늦어요" };
    expect(
      await resultOf({ toolName: "tool_call", input: { name: "slack_send_message", input } }),
    ).toEqual({ type: "json", value: { sent: true } });
    expect(sent).toEqual(["general: 10분 늦어요"]);
  });

  test("tells the model when the tool is unknown or the input does not fit", async () => {
    const unknown = { name: "slack_post", input: {} };
    expect(await errorOf({ toolName: "tool_call", input: unknown })).toContain("tool_search");

    const misfit = { name: "slack_send_message", input: { text: 1 } };
    expect(await errorOf({ toolName: "tool_call", input: misfit })).toContain("Invalid input");
    expect(sent).toEqual([]);
  });
});

describe("time", () => {
  test("tells the model the time in the user's time zone", async () => {
    await resetDatabase();
    llm.reset();
    const token = await signIn("minho@example.com", "민호");
    await client.me.$patch({ json: { timeZone: "Asia/Seoul" } }, bearer(token));
    const agentId = await createAgent(token);
    llm.reply = () =>
      Promise.resolve(llm.prompts.length === 1 ? { toolName: "time", input: {} } : "곧 점심이에요");

    await send(token, agentId, "지금 몇 시야?");
    await waitForIdle(agentId);

    expect(JSON.stringify(llm.prompts[1])).toMatch(/T\d{2}:\d{2}\+09:00\[Asia\/Seoul\]/);
  });
});

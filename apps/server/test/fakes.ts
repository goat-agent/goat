import { MockLanguageModelV4 } from "ai/test";
import type { Scraped } from "../src/lib/web/firecrawl";
import type { SearchResult } from "../src/lib/web/index";

type Prompt = Parameters<MockLanguageModelV4["doGenerate"]>[0]["prompt"];
type ToolCall = { toolName: string; input: object };
type Reply = (prompt: Prompt) => Promise<string | ToolCall>;

const defaultReply: Reply = () => Promise.resolve("ok");

export const llm = {
  prompts: [] as Prompt[],
  active: 0,
  maxActive: 0,
  reply: defaultReply,
  reset() {
    this.prompts = [];
    this.active = 0;
    this.maxActive = 0;
    this.reply = defaultReply;
  },
};

const usage = {
  inputTokens: { total: 1, noCache: 1, cacheRead: undefined, cacheWrite: undefined },
  outputTokens: { total: 1, text: 1, reasoning: undefined },
};

export const fakeModel = new MockLanguageModelV4({
  doGenerate: async ({ prompt }) => {
    llm.prompts.push(prompt);
    llm.active += 1;
    llm.maxActive = Math.max(llm.maxActive, llm.active);
    try {
      const reply = await llm.reply(prompt);
      if (typeof reply === "string") {
        return {
          content: [{ type: "text", text: reply }],
          finishReason: { unified: "stop", raw: undefined },
          usage,
          warnings: [],
        };
      }
      return {
        content: [
          {
            type: "tool-call",
            toolCallId: `call-${llm.prompts.length}`,
            toolName: reply.toolName,
            input: JSON.stringify(reply.input),
          },
        ],
        finishReason: { unified: "tool-calls", raw: undefined },
        usage,
        warnings: [],
      };
    } finally {
      llm.active -= 1;
    }
  },
});

const blankPage: Scraped = { title: "", url: "", markdown: "" };

export const webBackend = {
  queries: [] as string[],
  results: [] as SearchResult[],
  page: blankPage,
  reset() {
    this.queries = [];
    this.results = [];
    this.page = blankPage;
  },
};

export const fakeFirecrawl = {
  search: (query: string): Promise<SearchResult[]> => {
    webBackend.queries.push(query);
    return Promise.resolve(webBackend.results);
  },
  scrape: (url: string): Promise<Scraped> => Promise.resolve({ ...webBackend.page, url }),
};

export const mailbox = new Map<string, string>();

export class FakeResend {
  emails = {
    send: (email: { to: string; text: string }) => {
      mailbox.set(email.to, email.text);
      return Promise.resolve({ data: { id: crypto.randomUUID() }, error: null });
    },
  };
}

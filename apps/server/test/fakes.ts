import { MockLanguageModelV4 } from "ai/test";

type Prompt = Parameters<MockLanguageModelV4["doGenerate"]>[0]["prompt"];
type Reply = (prompt: Prompt) => Promise<string>;

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

export const fakeModel = new MockLanguageModelV4({
  doGenerate: async ({ prompt }) => {
    llm.prompts.push(prompt);
    llm.active += 1;
    llm.maxActive = Math.max(llm.maxActive, llm.active);
    try {
      const text = await llm.reply(prompt);
      return {
        content: [{ type: "text", text }],
        finishReason: { unified: "stop", raw: undefined },
        usage: {
          inputTokens: { total: 1, noCache: 1, cacheRead: undefined, cacheWrite: undefined },
          outputTokens: { total: 1, text: 1, reasoning: undefined },
        },
        warnings: [],
      };
    } finally {
      llm.active -= 1;
    }
  },
});

export const mailbox = new Map<string, string>();

export class FakeResend {
  emails = {
    send: (email: { to: string; text: string }) => {
      mailbox.set(email.to, email.text);
      return Promise.resolve({ data: { id: crypto.randomUUID() }, error: null });
    },
  };
}

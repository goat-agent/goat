import { tool } from "ai";
import { MockLanguageModelV4 } from "ai/test";
import { z } from "zod";
import type { TranscriptionHints } from "../src/lib/ai/providers";
import type { Handlers, Session, Setup } from "../src/lib/calls/openai";
import type { Fetch, Integration } from "../src/lib/integrations/integration";
import type { Scraped } from "../src/lib/web/firecrawl";
import type { SearchResult } from "../src/lib/web/index";

type Prompt = Parameters<MockLanguageModelV4["doGenerate"]>[0]["prompt"];
type ToolCall = { toolName: string; input: object };
type Reply = (prompt: Prompt) => Promise<string | ToolCall>;

const defaultReply: Reply = () => Promise.resolve("ok");

export const llm = {
  prompts: [] as Prompt[],
  toolNames: [] as string[][],
  active: 0,
  maxActive: 0,
  reply: defaultReply,
  reset() {
    this.prompts = [];
    this.toolNames = [];
    this.active = 0;
    this.maxActive = 0;
    this.reply = defaultReply;
  },
};

export const transcriber = {
  hints: [] as TranscriptionHints[],
  text: "",
  reset() {
    this.hints = [];
    this.text = "";
  },
};

export function fakeTranscribe(_audio: Uint8Array, hints: TranscriptionHints): Promise<string> {
  transcriber.hints.push(hints);
  return Promise.resolve(transcriber.text);
}

export const voiceService = {
  setups: [] as Setup[],
  handlers: undefined as Handlers | undefined,
  reports: [] as { taskId: string; text: string }[],
  reset() {
    this.setups = [];
    this.handlers = undefined;
    this.reports = [];
  },
};

export const fakeVoice = {
  start: (offer: string, setup: Setup): Promise<{ id: string; answer: string }> => {
    voiceService.setups.push(setup);
    return Promise.resolve({ id: "live_test", answer: `answer to ${offer}` });
  },
  attach: (_id: string, handlers: Handlers): Session => {
    voiceService.handlers = handlers;
    return {
      report: (taskId, text) => {
        voiceService.reports.push({ taskId, text });
      },
      close: () => {
        handlers.onClosed();
      },
    };
  },
};

const usage = {
  inputTokens: { total: 1, noCache: 1, cacheRead: undefined, cacheWrite: undefined },
  outputTokens: { total: 1, text: 1, reasoning: undefined },
};

// Input grows with the conversation the way a real model's does, so compaction has a size to read.
function usageFor(prompt: Prompt) {
  const conversation = prompt.filter((message) => message.role !== "system");
  const total = Math.ceil(JSON.stringify(conversation).length / 2.5);
  return {
    inputTokens: { total, noCache: total, cacheRead: undefined, cacheWrite: undefined },
    outputTokens: { total: 1, text: 1, reasoning: undefined },
  };
}

export const fakeModel = new MockLanguageModelV4({
  doGenerate: async ({ prompt, tools }) => {
    llm.prompts.push(prompt);
    llm.toolNames.push(tools?.map((candidate) => candidate.name) ?? []);
    llm.active += 1;
    llm.maxActive = Math.max(llm.maxActive, llm.active);
    try {
      const reply = await llm.reply(prompt);
      if (typeof reply === "string") {
        return {
          content: [{ type: "text", text: reply }],
          finishReason: { unified: "stop", raw: undefined },
          usage: usageFor(prompt),
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
        usage: usageFor(prompt),
        warnings: [],
      };
    } finally {
      llm.active -= 1;
    }
  },
});

export const summaries = {
  prompts: [] as Prompt[],
  reply: (_prompt: Prompt): Promise<string> =>
    Promise.resolve("summary of the earlier conversation"),
  reset() {
    this.prompts = [];
    this.reply = () => Promise.resolve("summary of the earlier conversation");
  },
};

export const fakeSummarizer = new MockLanguageModelV4({
  doGenerate: async ({ prompt }) => {
    summaries.prompts.push(prompt);
    return {
      content: [{ type: "text", text: await summaries.reply(prompt) }],
      finishReason: { unified: "stop", raw: undefined },
      usage,
      warnings: [],
    };
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

export const storedObjects = new Map<string, { data: Uint8Array; mediaType: string }>();

export const fakeStorage = {
  put: (key: string, data: Uint8Array, mediaType: string): Promise<void> => {
    storedObjects.set(key, { data, mediaType });
    return Promise.resolve();
  },
  get: (key: string): Promise<Uint8Array> => {
    const stored = storedObjects.get(key);
    return stored ? Promise.resolve(stored.data) : Promise.reject(new Error(`no object ${key}`));
  },
  remove: (key: string): Promise<void> => {
    storedObjects.delete(key);
    return Promise.resolve();
  },
  open: (key: string): Blob => {
    const stored = storedObjects.get(key);
    if (!stored) throw new Error(`no object ${key}`);
    return new Blob([stored.data], { type: stored.mediaType });
  },
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

type Account = { id: string; login: string };

export const oauth = {
  codes: new Map<
    string,
    { challenge: string; account: Account; expiresIn?: number; scope?: string }
  >(),
  accounts: new Map<string, Account>(),
  revoked: [] as string[],
  stale: new Set<string>(),
  refreshes: 0,
  rejectRefresh: false,
  reset() {
    this.codes.clear();
    this.accounts.clear();
    this.revoked = [];
    this.stale.clear();
    this.refreshes = 0;
    this.rejectRefresh = false;
  },
};

const server = Bun.serve({
  port: 0,
  fetch: async (request) => {
    const url = new URL(request.url);
    if (url.pathname === "/.well-known/oauth-authorization-server") {
      return Response.json({
        issuer: url.origin,
        authorization_endpoint: `${url.origin}/authorize`,
        token_endpoint: `${url.origin}/token`,
        revocation_endpoint: `${url.origin}/revoke`,
      });
    }
    const form = new URLSearchParams(request.method === "POST" ? await request.text() : "");
    if (url.pathname === "/token") return issueToken(form);
    if (url.pathname === "/revoke") {
      oauth.revoked.push(form.get("token") ?? "");
      return new Response(null, { status: 200 });
    }

    const bearer = request.headers.get("authorization")?.replace(/^Bearer /, "") ?? "";
    const account = oauth.stale.has(bearer) ? undefined : oauth.accounts.get(bearer);
    if (!account) return new Response(null, { status: 401 });
    if (url.pathname === "/user") return Response.json(account);
    if (url.pathname === "/issues") return Response.json({ issues: [`${account.login}'s issue`] });
    return new Response(null, { status: 404 });
  },
});

server.unref();

export const oauthIssuer = server.url.origin;

function issueToken(form: URLSearchParams): Response {
  if (form.get("client_id") !== "goat" || form.get("client_secret") !== "s3cret") {
    return Response.json({ error: "invalid_client" }, { status: 401 });
  }

  if (form.get("grant_type") === "refresh_token") {
    oauth.refreshes += 1;
    const account = oauth.accounts.get(form.get("refresh_token")?.replace(/^refresh-/, "") ?? "");
    if (oauth.rejectRefresh || !account) {
      return Response.json({ error: "invalid_grant" }, { status: 400 });
    }
    return Response.json({ access_token: grant(account), expires_in: 3600 });
  }

  const code = form.get("code") ?? "";
  const issued = oauth.codes.get(code);
  const verifier = form.get("code_verifier") ?? "";
  const challenge = new Bun.CryptoHasher("sha256").update(verifier).digest("base64url");
  if (!issued || issued.challenge !== challenge) {
    return Response.json({ error: "invalid_grant" }, { status: 400 });
  }
  oauth.codes.delete(code);
  const accessToken = grant(issued.account);
  return Response.json({
    access_token: accessToken,
    token_type: "Bearer",
    refresh_token: `refresh-${accessToken}`,
    ...(issued.expiresIn ? { expires_in: issued.expiresIn } : {}),
    ...(issued.scope ? { scope: issued.scope } : {}),
  });
}

const UserBody = z.object({ id: z.string(), login: z.string() });
const IssuesBody = z.object({ issues: z.array(z.string()) });

export const fakeIntegrations: Record<string, Integration> = {
  testhub: {
    name: "TestHub",
    description: "Code hosting for tests: repositories, issues, pull requests.",
    auth: {
      type: "oauth",
      issuer: oauthIssuer,
      client: { id: "goat", secret: "s3cret" },
      scopes: ["repo"],
      identify: async ({ fetch }) => {
        const user = UserBody.parse(await (await fetch(`${oauthIssuer}/user`)).json());
        return { id: user.id, name: user.login };
      },
    },
    tools: {
      testhub_list_issues: tool({
        description: "TestHub: list issues assigned to the user.",
        inputSchema: z.object({}),
        contextSchema: z.object({ fetch: z.custom<Fetch>() }),
        metadata: { scope: "repo" },
        execute: async (_, { context }) =>
          IssuesBody.parse(await (await context.fetch(`${oauthIssuer}/issues`)).json()),
      }),
      testhub_delete_repository: tool({
        description: "TestHub: delete a repository.",
        inputSchema: z.object({ name: z.string() }),
        metadata: { scope: "admin" },
        execute: ({ name }) => ({ deleted: name }),
      }),
    },
  },
  notes: {
    name: "Notes",
    description: "Pages, documents, meeting notes.",
    auth: {
      type: "oauth",
      issuer: oauthIssuer,
      client: { id: "goat", secret: "s3cret" },
      scopes: [],
    },
    tools: {},
  },
};

function grant(account: Account): string {
  const accessToken = `token-${account.login}-${crypto.randomUUID()}`;
  oauth.accounts.set(accessToken, account);
  return accessToken;
}

// Words map to buckets, so texts that share words are near each other. Two words given the same
// meaning in `meanings` share a bucket, which stands in for a paraphrase.
export const embedder = {
  documents: [] as string[],
  queries: [] as string[],
  isFailing: false,
  // Runs while documents are being embedded, as a change to the conversation would.
  onDocuments: undefined as (() => Promise<void>) | undefined,
  meanings: new Map<string, string>(),
  reset() {
    this.documents = [];
    this.queries = [];
    this.isFailing = false;
    this.onDocuments = undefined;
    this.meanings = new Map();
  },
};

function vectorOf(text: string): number[] {
  const vector = Array.from({ length: 768 }, () => 0);
  for (const word of text
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)) {
    const meaning = embedder.meanings.get(word) ?? word;
    let hash = 2166136261;
    for (const char of meaning) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619) >>> 0;
    vector[hash % 768] = (vector[hash % 768] ?? 0) + 1;
  }
  const length = Math.sqrt(vector.reduce((sum, x) => sum + x * x, 0)) || 1;
  return vector.map((x) => x / length);
}

export const fakeEmbeddings = {
  embeddingModel: "fake-embedder",
  embedDocuments: async (texts: readonly string[]): Promise<number[][]> => {
    if (embedder.isFailing) throw new Error("embedder is down");
    embedder.documents.push(...texts);
    await embedder.onDocuments?.();
    return texts.map(vectorOf);
  },
  embedQuery: (text: string): Promise<number[]> => {
    if (embedder.isFailing) return Promise.reject(new Error("embedder is down"));
    embedder.queries.push(text);
    return Promise.resolve(vectorOf(text));
  },
};

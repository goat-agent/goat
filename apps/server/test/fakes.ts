import { tool } from "ai";
import { MockLanguageModelV4 } from "ai/test";
import { z } from "zod";
import type { Fetch, Integration } from "../src/lib/integrations/integration";
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

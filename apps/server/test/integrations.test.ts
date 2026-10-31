import { beforeEach, describe, expect, test } from "bun:test";
import { eq } from "drizzle-orm";
import { db } from "../src/db";
import { users } from "../src/db/schema/auth";
import { connections } from "../src/db/schema/connections";
import { env } from "../src/env";
import { open, seal } from "../src/lib/integrations/credentials";
import { llm, oauth, oauthIssuer } from "./fakes";
import {
  bearer,
  client,
  codeFor,
  createAgent,
  listMessages,
  resetDatabase,
  send,
  signIn,
  status,
  waitForIdle,
} from "./helpers";

let token: string;
let userId: string;

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
  oauth.reset();
  token = await signIn("minho@example.com", "민호");
  const [user] = await db
    .select({ id: users.id })
    .from(users)
    .where(eq(users.email, "minho@example.com"));
  if (!user) throw new Error("no user");
  userId = user.id;
});

function param(url: URL, name: string): string {
  const value = url.searchParams.get(name);
  if (value === null) throw new Error(`no ${name} in ${url.href}`);
  return value;
}

async function startConnecting(integration = "testhub", agentId?: string): Promise<URL> {
  const response = await client.integrations[":id"].connect.$post(
    { param: { id: integration }, json: agentId ? { agentId } : {} },
    bearer(token),
  );
  if (!response.ok) throw new Error(`connect failed with ${response.status}`);
  return new URL((await response.json()).url);
}

async function callback(integration: string, query: Record<string, string>): Promise<URL> {
  const response = await client.integrations[":id"].callback.$get({
    param: { id: integration },
    query,
  });
  expect(response.status).toBe(302);
  return new URL(response.headers.get("location") ?? "");
}

function finish(back: URL, as = token) {
  return client.integrations[":id"].connections.$post(
    {
      param: { id: param(back, "integration") },
      json: { state: param(back, "state"), code: param(back, "code") },
    },
    bearer(as),
  );
}

type SignIn = { agentId?: string; expiresIn?: number; scope?: string };

async function signInAt(login: string, options: SignIn = {}) {
  const authorize = await startConnecting("testhub", options.agentId);
  const code = crypto.randomUUID();
  oauth.codes.set(code, {
    challenge: param(authorize, "code_challenge"),
    account: { id: `id-${login}`, login },
    ...(options.expiresIn ? { expiresIn: options.expiresIn } : {}),
    ...(options.scope ? { scope: options.scope } : {}),
  });
  return callback("testhub", { code, state: param(authorize, "state") });
}

async function connect(login: string, options: SignIn = {}) {
  const response = await finish(await signInAt(login, options));
  if (!response.ok) throw new Error(`finishing failed with ${response.status}`);
  return (await response.json()).connection;
}

async function listed() {
  const response = await client.integrations.$get(undefined, bearer(token));
  if (!response.ok) throw new Error(`list integrations failed with ${response.status}`);
  return (await response.json()).integrations;
}

function icon(id: string): string {
  return new URL(`/integrations/${id}/icon`, env.BETTER_AUTH_URL).href;
}

function rows() {
  return db.select().from(connections).where(eq(connections.userId, userId));
}

function toolOutput(prompt: number) {
  const message = llm.prompts[prompt]?.findLast((entry) => entry.role === "tool");
  const part = message?.role === "tool" ? message.content[0] : undefined;
  return part?.type === "tool-result" ? part.output : undefined;
}

function lastUserTexts(prompt: number) {
  return llm.prompts[prompt]
    ?.findLast((message) => message.role === "user")
    ?.content.map((part) => (part.type === "text" ? part.text : ""));
}

describe("connecting", () => {
  test("lists every integration the user can connect, with its logo", async () => {
    expect(await listed()).toEqual([
      { id: "testhub", name: "TestHub", iconUrl: icon("testhub"), connections: [] },
      { id: "notes", name: "Notes", iconUrl: icon("notes"), connections: [] },
    ]);

    const missing = await client.integrations[":id"].icon.$get({ param: { id: "testhub" } });
    expect(status(missing)).toBe(404);
  });

  test("sends a browser that started connecting back to the web app", async () => {
    await client.auth["email-code"].$post({ json: { email: "minho@example.com" } });
    const signedIn = await client.auth["sign-in"].$post({
      json: {
        email: "minho@example.com",
        code: codeFor("minho@example.com"),
        device: { installationId: crypto.randomUUID(), name: "Chrome · Mac", platform: "web" },
      },
    });
    if (!signedIn.ok) throw new Error(`sign-in failed with ${signedIn.status}`);
    const browser = (await signedIn.json()).token;

    const started = await client.integrations[":id"].connect.$post(
      { param: { id: "testhub" }, json: {} },
      bearer(browser),
    );
    if (!started.ok) throw new Error(`connect failed with ${started.status}`);
    const authorize = new URL((await started.json()).url);
    const back = await callback("testhub", { code: "code", state: param(authorize, "state") });

    expect(`${back.origin}${back.pathname}`).toBe(new URL("/connected", env.WEB_URL).href);
    expect(param(back, "integration")).toBe("testhub");
  });

  test("sends an app that started connecting back through goat://", async () => {
    const authorize = await startConnecting();
    const back = await callback("testhub", { code: "code", state: param(authorize, "state") });
    expect(`${back.protocol}//${back.host}`).toBe("goat://connected");
  });

  test("signs in with PKCE and an account choice, then keeps the account encrypted", async () => {
    const authorize = await startConnecting();
    expect(`${authorize.origin}${authorize.pathname}`).toBe(`${oauthIssuer}/authorize`);
    expect(param(authorize, "client_id")).toBe("goat");
    expect(param(authorize, "redirect_uri")).toBe(
      new URL("/integrations/testhub/callback", env.BETTER_AUTH_URL).href,
    );
    expect(param(authorize, "code_challenge_method")).toBe("S256");
    expect(param(authorize, "prompt")).toBe("select_account");
    expect(param(authorize, "scope")).toBe("repo");

    const back = await signInAt("mino");
    expect(`${back.protocol}//${back.host}`).toBe("goat://connected");
    expect(param(back, "integration")).toBe("testhub");
    expect(await rows()).toEqual([]);

    const response = await finish(back);
    expect(status(response)).toBe(201);
    const [connection] = await listed().then((list) => list[0]?.connections ?? []);
    expect(connection).toMatchObject({ accountName: "mino", needsReconnect: false });

    const [row] = await rows();
    expect(row?.credentials).not.toContain("token-mino");
    expect(open(row?.credentials ?? "").accessToken).toStartWith("token-mino-");
  });

  test("the same account again updates its row, and another account adds one", async () => {
    await connect("mino");
    await connect("mino");
    expect((await rows()).map((row) => row.accountName)).toEqual(["mino"]);

    await connect("mino_acme");
    expect((await rows()).map((row) => row.accountName)).toEqual(["mino", "mino_acme"]);
  });

  test("each attempt finishes once, only for its own integration", async () => {
    const authorize = await startConnecting();
    const wrongPath = await callback("notes", { code: "x", state: param(authorize, "state") });
    expect(wrongPath.href).toBe("goat://connected?integration=notes&error=expired");

    const back = await signInAt("mino");
    expect(status(await finish(back))).toBe(201);
    expect(status(await finish(back))).toBe(404);
  });

  test("a sign-in page sent to someone else cannot connect their account to the sender", async () => {
    const back = await signInAt("victim");
    const other = await signIn("other@example.com", "다른 사람");
    expect(status(await finish(back, other))).toBe(404);
    expect(await db.select().from(connections)).toEqual([]);
  });

  test("a refused sign-in returns to the app with the reason", async () => {
    const authorize = await startConnecting();
    const back = await callback("testhub", {
      error: "access_denied",
      state: param(authorize, "state"),
    });
    expect(back.href).toBe("goat://connected?integration=testhub&error=denied");
    expect(await rows()).toEqual([]);
  });

  test("refuses an integration that does not exist and another user's agent", async () => {
    const unknown = await client.integrations[":id"].connect.$post(
      { param: { id: "dropbox" }, json: {} },
      bearer(token),
    );
    expect(status(unknown)).toBe(404);

    const other = await signIn("other@example.com", "다른 사람");
    const agentId = await createAgent(other);
    const foreign = await client.integrations[":id"].connect.$post(
      { param: { id: "testhub" }, json: { agentId } },
      bearer(token),
    );
    expect(status(foreign)).toBe(404);
  });

  test("disconnecting revokes the token at the service and forgets the account", async () => {
    await connect("mino");
    const [row] = await rows();
    const credentials = open(row?.credentials ?? "");

    const response = await client.connections[":id"].$delete(
      { param: { id: row?.id ?? "" } },
      bearer(token),
    );
    expect(status(response)).toBe(204);
    expect(oauth.revoked).toEqual([credentials.refreshToken ?? ""]);
    expect(await rows()).toEqual([]);

    const again = await client.connections[":id"].$delete(
      { param: { id: row?.id ?? "" } },
      bearer(token),
    );
    expect(status(again)).toBe(404);
  });
});

describe("agent", () => {
  let agentId: string;

  beforeEach(async () => {
    agentId = await createAgent(token);
  });

  test("hears about connections when they change, not on every message", async () => {
    await connect("mino");
    await send(token, agentId, "안녕");
    await waitForIdle(agentId);
    expect(lastUserTexts(0)).toContain("Connected services: TestHub (mino).");

    await send(token, agentId, "또 안녕");
    await waitForIdle(agentId);
    expect(lastUserTexts(1)).toEqual(["또 안녕"]);

    await connect("mino_acme");
    await send(token, agentId, "하나 더");
    await waitForIdle(agentId);
    expect(lastUserTexts(2)).toContain("Connected services: TestHub (accounts: mino, mino_acme).");
  });

  test("shows a connect button and continues once the user connects", async () => {
    llm.reply = (prompt) => {
      if (llm.prompts.length === 1) {
        return Promise.resolve({ toolName: "connect", input: { integration: "TestHub" } });
      }
      const resumed = JSON.stringify(prompt).includes("Connected services");
      return Promise.resolve(resumed ? "연결됐어요" : "버튼을 띄웠어요");
    };
    await send(token, agentId, "이슈 봐줘");
    await waitForIdle(agentId);
    const shown = await listMessages(token, agentId);
    expect(shown.find((message) => message.connect)?.connect).toEqual([
      { integrationId: "testhub", name: "TestHub" },
    ]);

    await connect("mino", { agentId });
    await waitForIdle(agentId);
    const after = await listMessages(token, agentId);
    expect(after.at(-1)?.text).toBe("연결됐어요");
    expect(
      after.filter((message) => message.role === "user").map((message) => message.text),
    ).toEqual(["이슈 봐줘"]);
    expect(lastUserTexts(llm.prompts.length - 1)).toContain("Connected services: TestHub (mino).");
  });

  test("tool_call asks which account when there are several, then uses the chosen one", async () => {
    await connect("mino");
    await connect("mino_acme");
    const replies = [
      { toolName: "tool_call", input: { name: "testhub_list_issues", input: {} } },
      {
        toolName: "tool_call",
        input: { name: "testhub_list_issues", account: "mino_acme", input: {} },
      },
    ];
    llm.reply = () => Promise.resolve(replies[llm.prompts.length - 1] ?? "done");

    await send(token, agentId, "회사 이슈");
    await waitForIdle(agentId);
    expect(toolOutput(1)).toMatchObject({ type: "error-text" });
    expect(JSON.stringify(toolOutput(1))).toContain("2 connected accounts (mino, mino_acme)");
    expect(toolOutput(2)).toEqual({ type: "json", value: { issues: ["mino_acme's issue"] } });
  });

  test("tool_search offers a service that is not connected, with a way to connect it", async () => {
    const replies = [
      { toolName: "tool_search", input: { query: "meeting notes documents" } },
      { toolName: "tool_call", input: { name: "notes", input: {} } },
    ];
    llm.reply = () => Promise.resolve(replies[llm.prompts.length - 1] ?? "done");

    await send(token, agentId, "회의록 찾아줘");
    await waitForIdle(agentId);
    expect(JSON.stringify(toolOutput(1))).toContain("Notes (not connected)");
    expect(JSON.stringify(toolOutput(2))).toContain('Call connect with integration \\"notes\\"');
  });

  test("a found tool called by name still runs, through tool_call", async () => {
    await connect("mino");
    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 1 ? { toolName: "testhub_list_issues", input: {} } : "done",
      );

    await send(token, agentId, "내 이슈");
    await waitForIdle(agentId);
    expect(toolOutput(1)).toEqual({ type: "json", value: { issues: ["mino's issue"] } });
  });

  test("offers only the tools whose permission the user granted", async () => {
    await connect("mino", { scope: "repo" });
    const replies = [
      {
        toolName: "tool_call",
        input: { name: "testhub_delete_repository", input: { name: "goat" } },
      },
      { toolName: "tool_call", input: { name: "testhub_list_issues", input: {} } },
    ];
    llm.reply = () => Promise.resolve(replies[llm.prompts.length - 1] ?? "done");

    await send(token, agentId, "레포 지워줘");
    await waitForIdle(agentId);
    expect(JSON.stringify(toolOutput(1))).toContain("No tool named testhub_delete_repository");
    expect(toolOutput(2)).toEqual({ type: "json", value: { issues: ["mino's issue"] } });
  });

  test("refreshes an expiring token before using it", async () => {
    await connect("mino", { expiresIn: 30 });
    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 1
          ? { toolName: "tool_call", input: { name: "testhub_list_issues", input: {} } }
          : "done",
      );

    await send(token, agentId, "내 이슈");
    await waitForIdle(agentId);
    expect(oauth.refreshes).toBe(1);
    expect(toolOutput(1)).toEqual({ type: "json", value: { issues: ["mino's issue"] } });
  });

  test("a token refused before it expires is refreshed once and the call retried", async () => {
    await connect("mino");
    for (const accessToken of oauth.accounts.keys()) oauth.stale.add(accessToken);
    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 1
          ? { toolName: "tool_call", input: { name: "testhub_list_issues", input: {} } }
          : "done",
      );

    await send(token, agentId, "내 이슈");
    await waitForIdle(agentId);
    expect(oauth.refreshes).toBe(1);
    expect(toolOutput(1)).toEqual({ type: "json", value: { issues: ["mino's issue"] } });
    expect((await listed())[0]?.connections[0]?.needsReconnect).toBe(false);
  });

  test("asks to reconnect when the service refuses to refresh, and stops offering the account", async () => {
    await connect("mino", { expiresIn: 30 });
    oauth.rejectRefresh = true;
    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 1
          ? { toolName: "tool_call", input: { name: "testhub_list_issues", input: {} } }
          : "done",
      );

    await send(token, agentId, "내 이슈");
    await waitForIdle(agentId);
    expect(JSON.stringify(toolOutput(1))).toContain("needs to be connected again");
    expect((await listed())[0]?.connections[0]?.needsReconnect).toBe(true);

    await send(token, agentId, "다시");
    await waitForIdle(agentId);
    expect(lastUserTexts(llm.prompts.length - 1)).toContain("Connected services: none.");
  });
});

describe("credentials", () => {
  test("open only what seal wrote, untouched", () => {
    const sealed = seal({ type: "oauth", accessToken: "secret" });
    expect(sealed).toStartWith("v1.");
    expect(sealed).not.toContain("secret");
    expect(open(sealed)).toEqual({ type: "oauth", accessToken: "secret" });

    const middle = Math.floor(sealed.length / 2);
    const flipped = sealed[middle] === "A" ? "B" : "A";
    const tampered = `${sealed.slice(0, middle)}${flipped}${sealed.slice(middle + 1)}`;
    expect(() => open(tampered)).toThrow();
  });
});

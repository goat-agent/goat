import { eq, sql } from "drizzle-orm";
import { testClient } from "hono/testing";
import { db } from "../src/db";
import { agents, queuedMessages } from "../src/db/schema/agents";
import { users } from "../src/db/schema/auth";
import { app } from "../src/index";
import { mailbox } from "./fakes";

export const client = testClient(app);

export function status(response: { status: number }): number {
  return response.status;
}

export function bearer(token: string) {
  return { headers: { authorization: `Bearer ${token}` } };
}

export function codeFor(email: string): string {
  const code = mailbox.get(email)?.match(/\d{6}/)?.[0];
  if (!code) throw new Error(`no code was sent to ${email}`);
  return code;
}

export async function signIn(email: string, name?: string): Promise<string> {
  await client.auth["email-code"].$post({ json: { email } });
  const response = await client.auth["sign-in"].$post({ json: { email, code: codeFor(email) } });
  if (!response.ok) throw new Error(`sign-in failed with ${response.status}`);
  const { token } = await response.json();

  if (name) await setName(token, name);
  return token;
}

export async function setName(token: string, name: string): Promise<void> {
  const response = await client.me.$patch({ json: { name } }, bearer(token));
  if (!response.ok) throw new Error(`update me failed with ${response.status}`);
}

export async function createAgent(token: string): Promise<string> {
  const response = await client.agents.$post({ json: {} }, bearer(token));
  if (!response.ok) throw new Error(`create agent failed with ${response.status}`);
  const { agent } = await response.json();
  return agent.id;
}

export async function listMessages(token: string, agentId: string) {
  const response = await client.agents[":id"].messages.$get(
    { param: { id: agentId }, query: {} },
    bearer(token),
  );
  if (!response.ok) throw new Error(`list messages failed with ${response.status}`);
  const { messages } = await response.json();
  return messages;
}

export async function send(token: string, agentId: string, text: string) {
  return client.agents[":id"].messages.$post(
    { param: { id: agentId }, json: { text } },
    bearer(token),
  );
}

export async function waitFor(check: () => Promise<boolean> | boolean, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error("timed out waiting");
    await Bun.sleep(10);
  }
}

export function waitForIdle(agentId: string) {
  return waitFor(async () => {
    const [agent] = await db
      .select({ runOwner: agents.runOwner })
      .from(agents)
      .where(eq(agents.id, agentId));
    const [queued] = await db
      .select({ count: sql<number>`count(*)::int` })
      .from(queuedMessages)
      .where(eq(queuedMessages.agentId, agentId));
    return agent?.runOwner === null && queued?.count === 0;
  });
}

export async function resetDatabase() {
  await db.delete(users);
  mailbox.clear();
}

import { and, eq, gt, lt, sql } from "drizzle-orm";
import { db } from "../../db";
import { env } from "../../env";
import { connectionAttempts } from "../../db/schema/connections";
import { logger } from "../logger";
import { type Connection, bearerFetch, saveConnection } from "./connections";
import type { Credentials } from "./credentials";
import { integrations } from "./index";
import type { Account, Identity, Integration } from "./integration";
import { type McpSession, openSession } from "./mcp";
import { authorizationUrl, createState, createVerifier, exchangeCode } from "./oauth";

const ATTEMPT_LIFETIME = sql`interval '10 minutes'`;

export type Finished =
  | { status: "connected"; connection: Connection; agentId: string | undefined }
  | { status: "unknown" | "failed" };

const log = logger.child({ module: "authorize" });

export async function startConnecting(
  userId: string,
  integrationId: string,
  agentId: string | undefined,
  returnsToWeb: boolean,
): Promise<string | undefined> {
  const integration = integrations[integrationId];
  const client = integration?.auth.client;
  if (!integration || !client) return undefined;

  await db
    .delete(connectionAttempts)
    .where(lt(connectionAttempts.createdAt, sql`now() - ${ATTEMPT_LIFETIME}`));
  const state = createState();
  const verifier = createVerifier();
  await db
    .insert(connectionAttempts)
    .values({ state, userId, integrationId, agentId, codeVerifier: verifier, returnsToWeb });
  return authorizationUrl(integration.auth, client, {
    integrationId,
    state,
    verifier,
    ...(integration.mcp ? { resource: integration.mcp.url } : {}),
  });
}

// The browser that lands here may hold no goat session (apps sign in through a separate one), so
// the code goes back to the client that started the attempt, which finishes with its own session.
// Without a known attempt there is no way to tell which client that was; apps are the default.
export async function returnToClient(
  integrationId: string,
  response: { state?: string; code?: string; error?: string; iss?: string },
): Promise<URL> {
  const [attempt] = response.state
    ? await db
        .select({ returnsToWeb: connectionAttempts.returnsToWeb })
        .from(connectionAttempts)
        .where(current(response.state, integrationId))
    : [];
  const back = attempt?.returnsToWeb
    ? new URL("/connected", env.WEB_URL)
    : new URL("goat://connected");
  back.searchParams.set("integration", integrationId);
  const fail = (reason: "denied" | "expired" | "failed") => {
    back.searchParams.set("error", reason);
    return back;
  };

  if (!response.state) return fail("failed");
  if (!attempt) return fail("expired");
  if (response.error === "access_denied") return fail("denied");

  const issuer = integrations[integrationId]?.auth.issuer;
  const wrongIssuer = response.iss !== undefined && response.iss !== issuer;
  if (!response.code || response.error || wrongIssuer) return fail("failed");

  back.searchParams.set("state", response.state);
  back.searchParams.set("code", response.code);
  return back;
}

export async function finishConnecting(
  userId: string,
  integrationId: string,
  response: { state: string; code: string },
): Promise<Finished> {
  const [attempt] = await db
    .delete(connectionAttempts)
    .where(and(current(response.state, integrationId), eq(connectionAttempts.userId, userId)))
    .returning();
  if (!attempt) return { status: "unknown" };

  const integration = integrations[integrationId];
  const client = integration?.auth.client;
  if (!integration || !client) return { status: "unknown" };

  try {
    const { credentials, claims } = await exchangeCode(integration.auth, client, {
      integrationId,
      code: response.code,
      verifier: attempt.codeVerifier,
      ...(integration.mcp ? { resource: integration.mcp.url } : {}),
    });
    const account = await identify(integration, credentials, claims);
    const connection = await saveConnection(userId, integrationId, account, credentials);
    return { status: "connected", connection, agentId: attempt.agentId ?? undefined };
  } catch (error) {
    log.error({ err: error, integrationId }, "connecting failed");
    return { status: "failed" };
  }
}

function current(state: string, integrationId: string) {
  return and(
    eq(connectionAttempts.state, state),
    eq(connectionAttempts.integrationId, integrationId),
    gt(connectionAttempts.createdAt, sql`now() - ${ATTEMPT_LIFETIME}`),
  );
}

async function identify(
  integration: Integration,
  credentials: Credentials,
  claims: Identity["claims"],
): Promise<Account> {
  const fetch = bearerFetch(credentials.accessToken);
  let session: McpSession | undefined;
  const callTool: Identity["callTool"] = async (name, input) => {
    if (!integration.mcp) throw new Error(`${integration.name} has no MCP server`);
    session ??= await openSession(integration.mcp.url, fetch);
    return session.callTool(name, input);
  };

  try {
    if (integration.auth.identify) {
      return await integration.auth.identify({ fetch, callTool, ...(claims ? { claims } : {}) });
    }
    if (claims) return { id: claims.sub, name: claims.email ?? claims.name ?? claims.sub };
    throw new Error(`${integration.name} did not say whose account this is`);
  } finally {
    await session?.close();
  }
}

import { and, asc, eq } from "drizzle-orm";
import { db } from "../../db";
import { connections } from "../../db/schema/connections";
import { logger } from "../logger";
import { type Credentials, open, seal } from "./credentials";
import { integrations } from "./index";
import type { Account, Fetch, Integration } from "./integration";
import { GrantRejected, refresh, revoke } from "./oauth";

export type Connection = typeof connections.$inferSelect;

export class ReconnectNeeded extends Error {}

const REFRESH_MARGIN_MS = 60_000;

export const NOTE_PREFIX = "Connected services:";

const log = logger.child({ module: "connections" });

export function listConnections(userId: string): Promise<Connection[]> {
  return db
    .select()
    .from(connections)
    .where(eq(connections.userId, userId))
    .orderBy(asc(connections.createdAt));
}

export function matchIntegration(name: string): string | undefined {
  const wanted = simplify(name);
  return Object.entries(integrations).find(
    ([id, integration]) => simplify(id) === wanted || simplify(integration.name) === wanted,
  )?.[0];
}

export function noteFor(rows: Connection[]): string {
  const accounts = new Map<string, string[]>();
  for (const row of rows) {
    const integration = integrations[row.integrationId];
    if (!integration || row.needsReconnect) continue;
    accounts.set(integration.name, [...(accounts.get(integration.name) ?? []), row.accountName]);
  }
  if (accounts.size === 0) return `${NOTE_PREFIX} none.`;
  const services = [...accounts].map(([name, names]) =>
    names.length > 1 ? `${name} (accounts: ${names.join(", ")})` : `${name} (${names[0]})`,
  );
  return `${NOTE_PREFIX} ${services.join(", ")}.`;
}

export async function saveConnection(
  userId: string,
  integrationId: string,
  account: Account,
  credentials: Credentials,
): Promise<Connection> {
  const sealed = seal(await withKeptRefreshToken(userId, integrationId, account, credentials));
  const [row] = await db
    .insert(connections)
    .values({
      userId,
      integrationId,
      accountId: account.id,
      accountName: account.name,
      credentials: sealed,
    })
    .onConflictDoUpdate({
      target: [connections.userId, connections.integrationId, connections.accountId],
      set: { accountName: account.name, credentials: sealed, needsReconnect: false },
    })
    .returning();
  if (!row) throw new Error("connection upsert returned no row");
  return row;
}

// Services may skip the refresh token when the same account signs in again; the earlier one stays valid.
async function withKeptRefreshToken(
  userId: string,
  integrationId: string,
  account: Account,
  credentials: Credentials,
): Promise<Credentials> {
  if (credentials.refreshToken) return credentials;
  const [existing] = await db
    .select({ credentials: connections.credentials })
    .from(connections)
    .where(
      and(
        eq(connections.userId, userId),
        eq(connections.integrationId, integrationId),
        eq(connections.accountId, account.id),
      ),
    );
  const kept = existing ? open(existing.credentials).refreshToken : undefined;
  return kept ? { ...credentials, refreshToken: kept } : credentials;
}

export async function removeConnection(userId: string, connectionId: string): Promise<boolean> {
  const [row] = await db
    .select()
    .from(connections)
    .where(and(eq(connections.id, connectionId), eq(connections.userId, userId)));
  if (!row) return false;

  const integration = integrations[row.integrationId];
  if (integration?.auth.client) {
    await revoke(integration.auth, integration.auth.client, open(row.credentials)).catch(
      (error: unknown) => log.warn({ err: error, connectionId }, "revocation failed"),
    );
  }
  await db.delete(connections).where(eq(connections.id, row.id));
  return true;
}

export function authorizedFetch(connection: Connection): Fetch {
  let sealed = connection.credentials;
  let refreshing: Promise<string> | undefined;

  // Requests in flight together share one refresh, so a rotating refresh token is spent once.
  const refreshed = (): Promise<string> =>
    (refreshing ??= renew().finally(() => {
      refreshing = undefined;
    }));

  const renew = async (): Promise<string> => {
    const credentials = open(sealed);
    const integration = integrationOf(connection);
    if (!integration.auth.client) throw reconnect(connection);
    try {
      const renewed = seal(
        await refresh(integration.auth, integration.auth.client, credentials, integration.mcp?.url),
      );
      const updated = await db
        .update(connections)
        .set({ credentials: renewed })
        .where(and(eq(connections.id, connection.id), eq(connections.credentials, sealed)))
        .returning({ id: connections.id });
      // Another run refreshed first; its token is the one that still works.
      sealed = updated.length > 0 ? renewed : await reload(connection);
      return open(sealed).accessToken;
    } catch (error) {
      if (!(error instanceof GrantRejected)) throw error;
      const latest = await reload(connection);
      if (latest !== sealed) {
        sealed = latest;
        return open(sealed).accessToken;
      }
      await markNeedsReconnect(connection);
      throw reconnect(connection);
    }
  };

  const token = (): Promise<string> => {
    const credentials = open(sealed);
    const expiresAt = credentials.expiresAt ? Date.parse(credentials.expiresAt) : Infinity;
    if (expiresAt - Date.now() > REFRESH_MARGIN_MS) return Promise.resolve(credentials.accessToken);
    return refreshed();
  };

  const refused = async (): Promise<never> => {
    await markNeedsReconnect(connection);
    throw reconnect(connection);
  };

  return async (input, init) => {
    const response = await fetch(signed(input, init, await token()));
    if (response.status !== 401) return response;

    // A token can stop working before it expires; one refresh tells that apart from a revoked grant.
    if (!open(sealed).refreshToken) return refused();
    const second = await fetch(signed(input, init, await refreshed()));
    return second.status === 401 ? refused() : second;
  };
}

export function bearerFetch(accessToken: string): Fetch {
  return (input, init) => fetch(signed(input, init, accessToken));
}

function signed(input: string | URL | Request, init: RequestInit | undefined, token: string) {
  const request =
    input instanceof Request
      ? new Request(input, init)
      : new Request(typeof input === "string" ? input : input.href, init);
  request.headers.set("authorization", `Bearer ${token}`);
  return request;
}

export async function markNeedsReconnect(connection: Connection): Promise<void> {
  await db
    .update(connections)
    .set({ needsReconnect: true })
    .where(eq(connections.id, connection.id));
}

function integrationOf(connection: Connection): Integration {
  const integration = integrations[connection.integrationId];
  if (!integration) throw new Error(`unknown integration ${connection.integrationId}`);
  return integration;
}

function reconnect(connection: Connection): ReconnectNeeded {
  const name = integrations[connection.integrationId]?.name ?? connection.integrationId;
  return new ReconnectNeeded(
    `The ${name} account ${connection.accountName} needs to be connected again. ` +
      `Call connect with integration "${connection.integrationId}".`,
  );
}

async function reload(connection: Connection): Promise<string> {
  const [row] = await db
    .select({ credentials: connections.credentials })
    .from(connections)
    .where(eq(connections.id, connection.id));
  if (!row) throw reconnect(connection);
  return row.credentials;
}

function simplify(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]/g, "");
}

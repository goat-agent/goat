import { tool, type ToolSet } from "ai";
import { z } from "zod";
import { logger } from "../logger";
import { authorizedFetch, type Connection, listConnections } from "./connections";
import { open } from "./credentials";
import { integrations } from "./index";
import type { Fetch, Integration } from "./integration";
import { type McpSession, openSession } from "./mcp";

type Tool = ToolSet[string];

export type Catalog = {
  search: () => Promise<ToolSet>;
  resolve: (name: string, account?: string) => Promise<Tool>;
  reload: () => Promise<void>;
  close: () => Promise<void>;
};

type Entry = { integrationId: string; connection: Connection; tool: Tool };

type Loaded = {
  byName: Map<string, Entry[]>;
  unconnected: Set<string>;
  unreachable: Set<string>;
};

const CLOSE_TIMEOUT_MS = 5_000;

const log = logger.child({ module: "catalog" });

/** With `readOnly`, only tools that change nothing are reachable and nothing offers to connect. */
export function openCatalog(userId: string, { readOnly = false } = {}): Catalog {
  const sessions: McpSession[] = [];
  let loading: Promise<Loaded> | undefined;
  const load = () => (loading ??= loadFor(userId, sessions, readOnly));

  const search = async (): Promise<ToolSet> => {
    const { byName, unconnected } = await load();
    const found: ToolSet = {};
    for (const [name, entries] of byName) {
      const first = entries[0];
      if (first) found[name] = first.tool;
    }
    for (const id of unconnected) {
      const integration = integrations[id];
      if (integration) found[id] = placeholder(id, integration);
    }
    return found;
  };

  const resolve = async (name: string, account?: string): Promise<Tool> => {
    const { byName, unconnected, unreachable } = await load();
    const entries = byName.get(name);
    if (!entries?.[0]) {
      if (unconnected.has(name)) throw new Error(notConnected(name));
      const down = [...unreachable].find((id) => name.startsWith(`${id}_`));
      if (down) {
        const service = integrations[down]?.name ?? down;
        throw new Error(`${service} could not be reached just now; try again later.`);
      }
      throw new Error(`No tool named ${name}; find it with tool_search.`);
    }
    const integrationName = integrations[entries[0].integrationId]?.name ?? name;
    const accounts = entries.map((entry) => entry.connection.accountName);
    if (account !== undefined) {
      const match = entries.find((entry) => entry.connection.accountName === account);
      if (!match) {
        throw new Error(
          `No ${integrationName} account named ${account}. Accounts: ${accounts.join(", ")}.`,
        );
      }
      return match.tool;
    }
    if (entries.length > 1) {
      throw new Error(
        `${integrationName} has ${entries.length} connected accounts (${accounts.join(", ")}); pass account.`,
      );
    }
    return entries[0].tool;
  };

  // A server that never answers must not hold the run open.
  const close = async (): Promise<void> => {
    const closing = sessions.splice(0).map((session) => session.close());
    await Promise.race([Promise.allSettled(closing), Bun.sleep(CLOSE_TIMEOUT_MS)]);
  };

  const reload = async (): Promise<void> => {
    loading = undefined;
    await close();
  };

  return { search, resolve, reload, close };
}

async function loadFor(userId: string, sessions: McpSession[], readOnly: boolean): Promise<Loaded> {
  const rows = await listConnections(userId);
  const working = rows.filter((row) => !row.needsReconnect && integrations[row.integrationId]);
  const byName = new Map<string, Entry[]>();
  const unreachable = new Set<string>();

  await Promise.all(
    working.map(async (connection) => {
      const integration = integrations[connection.integrationId];
      if (!integration) return;
      try {
        const tools = await toolsOf(connection, integration, sessions, readOnly);
        for (const [name, found] of Object.entries(tools)) {
          const entry = { integrationId: connection.integrationId, connection, tool: found };
          byName.set(name, [...(byName.get(name) ?? []), entry]);
        }
      } catch (error) {
        log.warn({ err: error, connectionId: connection.id }, "could not load connection tools");
        unreachable.add(connection.integrationId);
      }
    }),
  );

  const connected = new Set(working.map((row) => row.integrationId));
  const unconnected = new Set(
    readOnly ? [] : Object.keys(integrations).filter((id) => !connected.has(id)),
  );
  return { byName, unconnected, unreachable };
}

async function toolsOf(
  connection: Connection,
  integration: Integration,
  sessions: McpSession[],
  readOnly: boolean,
): Promise<ToolSet> {
  const fetch = authorizedFetch(connection);
  const found: ToolSet = {};
  const allowed = Object.entries(integration.mcp?.tools ?? {}).flatMap(([name, effect]) =>
    readOnly && effect !== "read" ? [] : [name],
  );
  if (integration.mcp && allowed.length > 0) {
    const session = await openSession(integration.mcp.url, fetch);
    sessions.push(session);
    const tools = await session.tools(allowed);
    for (const [name, mcpTool] of Object.entries(tools)) {
      found[exposedName(connection.integrationId, name)] = mcpTool;
    }
  }
  // Consent screens let the user turn off single permissions, so a tool whose scope was not
  // granted stays out of reach.
  const granted = open(connection.credentials).scope?.split(" ");
  for (const [name, own] of Object.entries(integration.tools ?? {})) {
    if (readOnly && own.metadata?.effect !== "read") continue;
    const scope = own.metadata?.scope;
    if (granted && typeof scope === "string" && !granted.includes(scope)) continue;
    found[name] = withFetch(own, fetch);
  }
  return found;
}

export function exposedName(integrationId: string, name: string): string {
  const simple = name.toLowerCase().replace(/[^a-z0-9_]+/g, "_");
  return simple.startsWith(`${integrationId}_`) ? simple : `${integrationId}_${simple}`;
}

function withFetch(own: Tool, fetch: Fetch): Tool {
  const { execute } = own;
  if (!execute) return own;
  return {
    ...own,
    execute: (input, options) => {
      const output: unknown = execute(input, { ...options, context: { fetch } });
      return output;
    },
  };
}

function placeholder(id: string, integration: Integration): Tool {
  return tool({
    description: `${integration.name} (not connected): ${integration.description} To use it, call connect with integration "${id}".`,
    inputSchema: z.object({}),
  });
}

function notConnected(id: string): string {
  const name = integrations[id]?.name ?? id;
  return `${name} is not connected. Call connect with integration "${id}" to show the user a button.`;
}

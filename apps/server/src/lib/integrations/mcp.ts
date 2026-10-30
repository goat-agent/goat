import { type CallToolResult, createMCPClient, type ListToolsResult } from "@ai-sdk/mcp";
import type { ToolSet } from "ai";
import { z } from "zod";
import { logger } from "../logger";
import type { Fetch } from "./integration";

export type McpSession = {
  tools: (allowed: readonly string[]) => Promise<ToolSet>;
  callTool: (name: string, input?: Record<string, unknown>) => Promise<unknown>;
  close: () => Promise<void>;
};

const TextContent = z.array(z.object({ type: z.string(), text: z.string().optional() }));

const log = logger.child({ module: "mcp" });

export async function openSession(url: string, sign: Fetch): Promise<McpSession> {
  // The transport wants the global fetch's type, which in Bun also carries preconnect.
  const fetch = Object.assign(sign, { preconnect: globalThis.fetch.preconnect });
  const client = await createMCPClient({ transport: { type: "http", url, fetch } });

  const tools = async (allowed: readonly string[]): Promise<ToolSet> => {
    const listed = await listAll((options) => client.listTools(options));
    const kept = listed.tools.filter((tool) => allowed.includes(tool.name));
    const missing = allowed.filter((name) => !listed.tools.some((tool) => tool.name === name));
    if (missing.length > 0) log.warn({ url, missing }, "allowed MCP tools are missing");
    return client.toolsFromDefinitions({ ...listed, tools: kept });
  };

  const callTool = async (name: string, input: Record<string, unknown> = {}): Promise<unknown> => {
    const result = await client.callTool({ name, arguments: input });
    if (result.isError) throw new Error(`${name} failed: ${textOf(result)}`);
    return result.structuredContent ?? parse(textOf(result));
  };

  return { tools, callTool, close: () => client.close() };
}

async function listAll(
  list: (options?: { params?: { cursor?: string } }) => Promise<ListToolsResult>,
): Promise<ListToolsResult> {
  const first = await list();
  const tools = [...first.tools];
  let cursor = first.nextCursor;
  while (cursor) {
    const page = await list({ params: { cursor } });
    tools.push(...page.tools);
    cursor = page.nextCursor;
  }
  return { ...first, tools, nextCursor: undefined };
}

function textOf(result: CallToolResult): string {
  const content = TextContent.safeParse(result.content);
  return content.success ? content.data.flatMap((part) => part.text ?? []).join("\n") : "";
}

function parse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

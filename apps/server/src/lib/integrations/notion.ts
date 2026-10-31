import { z } from "zod";
import { env } from "../../env";
import type { Integration } from "./integration";

const Self = z.object({
  workspace: z.object({ id: z.string(), name: z.string() }),
  user: z.object({ id: z.string(), name: z.string().optional(), email: z.string().optional() }),
});

export const notion = {
  name: "Notion",
  description: "Pages, databases, docs, wiki, meeting notes, comments.",
  auth: {
    type: "oauth",
    issuer: "https://mcp.notion.com",
    client: env.NOTION_CLIENT_ID ? { id: env.NOTION_CLIENT_ID } : undefined,
    scopes: [],
    // Notion's MCP tokens work only with Notion MCP, not with its REST API.
    identify: async ({ callTool }) => {
      const result = await callTool("notion-get-self");
      const { workspace, user } = Self.parse(
        typeof result === "object" && result && "self" in result ? result.self : result,
      );
      return {
        id: `${workspace.id}:${user.id}`,
        name: [workspace.name, user.email, user.name].find(Boolean) ?? workspace.id,
      };
    },
  },
  mcp: {
    url: "https://mcp.notion.com/mcp",
    tools: {
      "notion-search": "read",
      "notion-fetch": "read",
      "notion-create-pages": "write",
      "notion-update-page": "write",
      "notion-get-comments": "read",
      "notion-create-comment": "write",
      "notion-get-users": "read",
      "notion-query-data-sources": "read",
    },
  },
} satisfies Integration;

import { env } from "../../env";
import type { Integration } from "./integration";

export const linear = {
  name: "Linear",
  description: "Issue tracking: issues, projects, cycles, comments, team work.",
  auth: {
    type: "oauth",
    issuer: "https://mcp.linear.app",
    client: env.LINEAR_CLIENT_ID ? { id: env.LINEAR_CLIENT_ID } : undefined,
    // openid and email make the token response carry an ID token that names the account.
    scopes: ["read", "write", "openid", "email"],
  },
  mcp: {
    url: "https://mcp.linear.app/mcp",
    tools: [
      "list_issues",
      "get_issue",
      "save_issue",
      "list_comments",
      "save_comment",
      "list_projects",
      "get_project",
      "list_teams",
      "list_users",
      "get_user",
      "list_issue_statuses",
      "list_issue_labels",
      "list_cycles",
      "list_documents",
      "get_document",
    ],
  },
} satisfies Integration;

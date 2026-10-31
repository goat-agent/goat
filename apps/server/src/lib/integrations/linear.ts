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
    tools: {
      list_issues: "read",
      get_issue: "read",
      save_issue: "write",
      list_comments: "read",
      save_comment: "write",
      list_projects: "read",
      get_project: "read",
      list_teams: "read",
      list_users: "read",
      get_user: "read",
      list_issue_statuses: "read",
      list_issue_labels: "read",
      list_cycles: "read",
      list_documents: "read",
      get_document: "read",
    },
  },
} satisfies Integration;

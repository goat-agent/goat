import { z } from "zod";
import { env } from "../../env";
import type { Integration } from "./integration";

const User = z.object({ id: z.number(), login: z.string() });

export const github = {
  name: "GitHub",
  description: "Code hosting: repositories, issues, pull requests, code review, commits.",
  auth: {
    type: "oauth",
    issuer: "https://github.com/login/oauth",
    client: env.GITHUB_CLIENT_ID
      ? { id: env.GITHUB_CLIENT_ID, secret: env.GITHUB_CLIENT_SECRET }
      : undefined,
    scopes: ["repo", "read:user"],
    identify: async ({ fetch }) => {
      const response = await fetch("https://api.github.com/user");
      if (!response.ok) throw new Error(`GitHub /user failed with ${response.status}`);
      const user = User.parse(await response.json());
      return { id: String(user.id), name: user.login };
    },
  },
  mcp: {
    url: "https://api.githubcopilot.com/mcp/",
    tools: {
      get_me: "read",
      search_repositories: "read",
      get_file_contents: "read",
      search_code: "read",
      list_commits: "read",
      list_issues: "read",
      search_issues: "read",
      issue_read: "read",
      issue_write: "write",
      add_issue_comment: "write",
      list_pull_requests: "read",
      search_pull_requests: "read",
      pull_request_read: "read",
    },
  },
} satisfies Integration;

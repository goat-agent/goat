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
    tools: [
      "get_me",
      "search_repositories",
      "get_file_contents",
      "search_code",
      "list_commits",
      "list_issues",
      "search_issues",
      "issue_read",
      "issue_write",
      "add_issue_comment",
      "list_pull_requests",
      "search_pull_requests",
      "pull_request_read",
    ],
  },
} satisfies Integration;

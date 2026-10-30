import { z } from "zod";
import { env } from "../../env";
import type { Integration } from "./integration";

const AuthTest = z.object({
  ok: z.literal(true),
  team: z.string(),
  team_id: z.string(),
  user_id: z.string(),
});

export const slack = {
  name: "Slack",
  description: "Team chat: messages, channels, threads, direct messages, canvases.",
  auth: {
    type: "oauth",
    issuer: "https://mcp.slack.com",
    client: env.SLACK_CLIENT_ID
      ? { id: env.SLACK_CLIENT_ID, secret: env.SLACK_CLIENT_SECRET }
      : undefined,
    scopes: [
      "search:read.public",
      "search:read.private",
      "search:read.mpim",
      "search:read.im",
      "search:read.users",
      "channels:history",
      "groups:history",
      "mpim:history",
      "im:history",
      "channels:read",
      "groups:read",
      "mpim:read",
      "im:read",
      "chat:write",
      "users:read",
      "users:read.email",
      "canvases:read",
    ],
    identify: async ({ fetch }) => {
      const response = await fetch("https://slack.com/api/auth.test", { method: "POST" });
      const result = AuthTest.parse(await response.json());
      // One person is a different account in each workspace.
      return { id: `${result.team_id}:${result.user_id}`, name: result.team };
    },
  },
  mcp: {
    url: "https://mcp.slack.com/mcp",
    tools: [
      "slack_search_public_and_private",
      "slack_search_channels",
      "slack_search_users",
      "slack_read_channel",
      "slack_read_thread",
      "slack_read_user_profile",
      "slack_list_user_channels",
      "slack_read_canvas",
      "slack_send_message",
      "slack_schedule_message",
    ],
  },
} satisfies Integration;

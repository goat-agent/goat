import { env } from "../../env";
import type { Integration } from "./integration";

export const granola = {
  name: "Granola",
  description:
    "Meeting notes: search and read past meetings, summaries, action items, decisions, transcripts, folders.",
  auth: {
    type: "oauth",
    issuer: "https://mcp-auth.granola.ai",
    client: env.GRANOLA_CLIENT_ID ? { id: env.GRANOLA_CLIENT_ID } : undefined,
    // openid and email make the token response carry an ID token that names the account.
    scopes: ["openid", "email", "offline_access"],
  },
  mcp: {
    url: "https://mcp.granola.ai/mcp",
    tools: {
      query_granola_meetings: "read",
      list_meetings: "read",
      get_meetings: "read",
      get_meeting_transcript: "read",
      list_meeting_folders: "read",
      get_meeting_attachment: "read",
    },
  },
} satisfies Integration;

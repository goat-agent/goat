import { z } from "zod";
import { env } from "../../env";
import { outlookCalendarTools } from "../ai/tools/outlook-calendar";
import { outlookMailTools } from "../ai/tools/outlook-mail";
import { microsoftTodoTools } from "../ai/tools/microsoft-todo";
import { oneDriveTools } from "../ai/tools/onedrive";
import type { Integration } from "./integration";

const User = z.object({
  id: z.string(),
  mail: z.string().nullish(),
  userPrincipalName: z.string(),
});

const GRAPH = "https://graph.microsoft.com/";

export const microsoft = {
  name: "Microsoft 365",
  description: "Outlook mail, Outlook calendar events, OneDrive files, and Microsoft To Do tasks.",
  auth: {
    type: "oauth",
    issuer: "https://login.microsoftonline.com/common/v2.0",
    client: env.MICROSOFT_CLIENT_ID
      ? { id: env.MICROSOFT_CLIENT_ID, secret: env.MICROSOFT_CLIENT_SECRET }
      : undefined,
    scopes: [
      "openid",
      "email",
      "profile",
      "offline_access",
      "User.Read",
      "Mail.ReadWrite",
      "Mail.Send",
      "Calendars.ReadWrite",
      "Files.ReadWrite",
      "Tasks.ReadWrite",
    ],
    identify: async ({ fetch }) => {
      const response = await fetch(`${GRAPH}v1.0/me?$select=id,mail,userPrincipalName`);
      if (!response.ok) throw new Error(`Microsoft Graph /me failed with ${response.status}`);
      const user = User.parse(await response.json());
      return { id: user.id, name: user.mail ?? user.userPrincipalName };
    },
  },
  tools: {
    ...outlookMailTools,
    ...outlookCalendarTools,
    ...oneDriveTools,
    ...microsoftTodoTools,
  },
} satisfies Integration;

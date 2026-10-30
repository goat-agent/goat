import { env } from "../../env";
import { gmailTools } from "../ai/tools/gmail";
import { googleScopes } from "../ai/tools/google";
import { googleCalendarTools } from "../ai/tools/google-calendar";
import { googleDocsTools } from "../ai/tools/google-docs";
import { googleDriveTools } from "../ai/tools/google-drive";
import { googleSheetsTools } from "../ai/tools/google-sheets";
import { googleSlidesTools } from "../ai/tools/google-slides";
import { googleTasksTools } from "../ai/tools/google-tasks";
import { youtubeTools } from "../ai/tools/youtube";
import type { Integration } from "./integration";

export const google = {
  name: "Google Workspace",
  description:
    "Gmail email, Google Calendar events, Google Tasks to-dos, Google Drive files, Docs, Sheets, Slides, and YouTube.",
  auth: {
    type: "oauth",
    issuer: "https://accounts.google.com",
    client: env.GOOGLE_CLIENT_ID
      ? { id: env.GOOGLE_CLIENT_ID, secret: env.GOOGLE_CLIENT_SECRET }
      : undefined,
    scopes: ["openid", "email", ...Object.values(googleScopes)],
    // Google issues a refresh token only for offline access, and again only after consent.
    authorizeParams: {
      access_type: "offline",
      prompt: "select_account consent",
      include_granted_scopes: "true",
    },
  },
  tools: {
    ...gmailTools,
    ...googleCalendarTools,
    ...googleTasksTools,
    ...googleDriveTools,
    ...googleDocsTools,
    ...googleSheetsTools,
    ...googleSlidesTools,
    ...youtubeTools,
  },
} satisfies Integration;

import { z } from "zod";
import { env } from "../../env";
import type { Integration } from "./integration";

const CurrentAccount = z.object({
  account_id: z.string(),
  email: z.string(),
});

export const dropbox = {
  name: "Dropbox",
  description:
    "Cloud files: browse folders, search files, read documents, create files and folders, copy, move, share links.",
  auth: {
    type: "oauth",
    issuer: "https://www.dropbox.com",
    client: env.DROPBOX_CLIENT_ID
      ? { id: env.DROPBOX_CLIENT_ID, secret: env.DROPBOX_CLIENT_SECRET }
      : undefined,
    scopes: [
      "account_info.read",
      "files.metadata.read",
      "files.metadata.write",
      "files.content.read",
      "files.content.write",
      "sharing.read",
      "sharing.write",
    ],
    // Dropbox returns a refresh token only when asked for offline access.
    authorizeParams: { token_access_type: "offline" },
    identify: async ({ fetch }) => {
      const response = await fetch("https://api.dropboxapi.com/2/users/get_current_account", {
        method: "POST",
      });
      if (!response.ok)
        throw new Error(`Dropbox get_current_account failed with ${response.status}`);
      const account = CurrentAccount.parse(await response.json());
      return { id: account.account_id, name: account.email };
    },
  },
  mcp: {
    url: "https://mcp.dropbox.com/mcp",
    tools: {
      ListFolder: "read",
      GetFileMetadata: "read",
      GetFileContent: "read",
      GetMarkdown: "read",
      Search: "read",
      DownloadLink: "read",
      ListSharedLinks: "read",
      GetSharedLinkMetadata: "read",
      CreateFolder: "write",
      CreateFile: "write",
      CreateSharedLink: "write",
      Copy: "write",
      Move: "write",
      Delete: "write",
      CheckJobStatus: "read",
    },
  },
} satisfies Integration;

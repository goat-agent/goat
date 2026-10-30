import { z } from "zod";
import { env } from "../../env";
import type { Integration } from "./integration";

const CanvaUser = z.object({ team_user: z.object({ user_id: z.string() }) });
const CanvaProfile = z.object({ profile: z.object({ display_name: z.string() }) });

export const canva = {
  name: "Canva",
  description:
    "Design: find and open designs, create and generate designs, presentations, social posts, export to PDF or image, folders, comments, uploads.",
  auth: {
    type: "oauth",
    issuer: "https://mcp.canva.com",
    client: env.CANVA_CLIENT_ID ? { id: env.CANVA_CLIENT_ID } : undefined,
    scopes: [
      "profile:read",
      "design:meta:read",
      "design:content:read",
      "design:content:write",
      "folder:read",
      "folder:write",
      "comment:read",
      "comment:write",
      "asset:read",
      "asset:write",
    ],
    identify: async ({ fetch }) => {
      const get = async (path: string): Promise<unknown> => {
        const response = await fetch(`https://api.canva.com/rest/v1/${path}`);
        if (!response.ok) throw new Error(`Canva ${path} failed with ${response.status}`);
        return response.json();
      };
      const { team_user: user } = CanvaUser.parse(await get("users/me"));
      const { profile } = CanvaProfile.parse(await get("users/me/profile"));
      return { id: user.user_id, name: profile.display_name };
    },
  },
  mcp: {
    url: "https://mcp.canva.com/mcp",
    tools: [
      "search-designs",
      "get-design",
      "get-design-pages",
      "get-design-content",
      "get-presenter-notes",
      "create-design",
      "generate-design",
      "create-design-from-candidate",
      "get-create-design-async-job",
      "copy-design",
      "resize-design",
      "get-export-formats",
      "export-design",
      "list-folder-items",
      "search-folders",
      "create-folder",
      "move-item-to-folder",
      "list-comments",
      "list-replies",
      "comment-on-design",
      "reply-to-comment",
      "get-assets",
      "upload-asset-from-url",
    ],
  },
} satisfies Integration;

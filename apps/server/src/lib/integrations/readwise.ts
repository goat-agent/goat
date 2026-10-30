import { env } from "../../env";
import type { Integration } from "./integration";

export const readwise = {
  name: "Readwise",
  description:
    "Reading and highlights: saved articles, Reader documents, tags, highlights, notes, daily review.",
  auth: {
    type: "oauth",
    issuer: "https://readwise.io/o/",
    client: env.READWISE_CLIENT_ID ? { id: env.READWISE_CLIENT_ID } : undefined,
    // openid makes the token response carry an ID token that names the account.
    scopes: ["openid", "read", "write"],
  },
  mcp: {
    url: "https://mcp2.readwise.io/mcp",
    tools: [
      "readwise_search_highlights",
      "readwise_list_highlights",
      "readwise_create_highlights",
      "readwise_update_highlight",
      "readwise_get_daily_review",
      "reader_search_documents",
      "reader_list_documents",
      "reader_get_document_details",
      "reader_create_document",
      "reader_move_documents",
      "reader_list_tags",
      "reader_add_tags_to_document",
      "reader_remove_tags_from_document",
      "reader_get_document_highlights",
      "reader_create_highlight",
      "reader_set_highlight_notes",
      "reader_add_tags_to_highlight",
      "reader_remove_tags_from_highlight",
    ],
  },
} satisfies Integration;

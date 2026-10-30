import { tool } from "ai";
import { z } from "zod";
import {
  callGoogle,
  GoogleConnection,
  googleScopes,
  page,
  readGoogleText,
  UNTRUSTED_NOTE,
} from "./google";

const API = "https://www.googleapis.com/drive/v3/files";
const FIELDS = "id,name,mimeType,modifiedTime,webViewLink";
const metadata = { scope: googleScopes.drive };

// Docs, Sheets, and Slides have no file of their own; Drive exports them as text.
const exports: Record<string, string> = {
  "application/vnd.google-apps.document": "text/plain",
  "application/vnd.google-apps.spreadsheet": "text/csv",
  "application/vnd.google-apps.presentation": "text/plain",
};

const File = z.object({
  id: z.string(),
  name: z.string(),
  mimeType: z.string(),
  modifiedTime: z.string().optional(),
  webViewLink: z.string().optional(),
});

const Files = z.object({ files: z.array(File).default([]) });

export const googleDriveTools = {
  google_drive_search: tool({
    description:
      "Google Drive: find files, including Docs, Sheets, and Slides, by words in their name or content.",
    inputSchema: z.object({
      query: z.string(),
      maxResults: z.int().min(1).max(50).default(20),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ query, maxResults }, { context, abortSignal }) => {
      const quoted = query.replace(/\\/g, "\\\\").replace(/'/g, "\\'");
      const url = new URL(API);
      url.search = new URLSearchParams({
        q: `fullText contains '${quoted}' and trashed = false`,
        fields: `files(${FIELDS})`,
        pageSize: String(maxResults),
        orderBy: "modifiedTime desc",
      }).toString();
      return Files.parse(await callGoogle(context.fetch, url, { signal: abortSignal }));
    },
  }),
  google_drive_read: tool({
    description: `Google Drive: read a file as text; Docs and Slides come as plain text, Sheets as CSV of the first sheet. Long files arrive in parts: when truncated is true, call again with offset set to nextOffset. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({ fileId: z.string(), offset: z.int().min(0).optional() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ fileId, offset }, { context, abortSignal }) => {
      const base = `${API}/${encodeURIComponent(fileId)}`;
      const file = File.parse(
        await callGoogle(context.fetch, `${base}?fields=${FIELDS}`, { signal: abortSignal }),
      );
      const exported = exports[file.mimeType];
      const readable = /^text\/|json$|xml$/.test(file.mimeType);
      if (!exported && !readable) {
        return { ...file, content: null, note: "This file is not text; open it from the link." };
      }
      const url = exported
        ? `${base}/export?mimeType=${encodeURIComponent(exported)}`
        : `${base}?alt=media`;
      const text = await readGoogleText(context.fetch, url, abortSignal);
      return { ...file, ...page(text, offset) };
    },
  }),
  google_drive_share: tool({
    description: "Google Drive: share a file with someone by email.",
    inputSchema: z.object({
      fileId: z.string(),
      email: z.email(),
      role: z.enum(["reader", "commenter", "writer"]),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ fileId, email, role }, { context, abortSignal }) => {
      await callGoogle(context.fetch, `${API}/${encodeURIComponent(fileId)}/permissions`, {
        method: "POST",
        body: { type: "user", role, emailAddress: email },
        signal: abortSignal,
      });
      return { shared: fileId, with: email, role };
    },
  }),
  google_drive_trash: tool({
    description: "Google Drive: move a file to the trash.",
    inputSchema: z.object({ fileId: z.string() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ fileId }, { context, abortSignal }) => {
      await callGoogle(context.fetch, `${API}/${encodeURIComponent(fileId)}`, {
        method: "PATCH",
        body: { trashed: true },
        signal: abortSignal,
      });
      return { trashed: fileId };
    },
  }),
};

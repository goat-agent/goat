import { tool } from "ai";
import { z } from "zod";
import { callGraph, MicrosoftConnection, page, readGraphText, UNTRUSTED_NOTE } from "./microsoft";

const API = "https://graph.microsoft.com/v1.0/me/drive";
const FIELDS = "id,name,size,lastModifiedDateTime,webUrl,file,folder";
const MAX_READ_BYTES = 5_000_000;

const File = z.object({
  id: z.string(),
  name: z.string(),
  size: z.number().optional(),
  lastModifiedDateTime: z.string().optional(),
  webUrl: z.string().optional(),
  file: z.object({ mimeType: z.string().optional() }).optional(),
  folder: z.object({}).optional(),
});

const Files = z.object({ value: z.array(File).default([]) });

const Link = z.object({ link: z.object({ webUrl: z.string() }) });

function summary(file: z.infer<typeof File>) {
  return {
    id: file.id,
    name: file.name,
    isFolder: file.folder !== undefined,
    size: file.size,
    modified: file.lastModifiedDateTime,
    link: file.webUrl,
  };
}

const readMetadata = { effect: "read" };

export const oneDriveTools = {
  onedrive_search: tool({
    description: "OneDrive: find files and folders by words in their name or content.",
    inputSchema: z.object({
      query: z.string(),
      maxResults: z.int().min(1).max(50).default(20),
    }),
    contextSchema: MicrosoftConnection,
    metadata: readMetadata,
    execute: async ({ query, maxResults }, { context, abortSignal }) => {
      const quoted = encodeURIComponent(query.replace(/'/g, "''"));
      const url = new URL(`${API}/root/search(q='${quoted}')`);
      url.search = new URLSearchParams({ $top: String(maxResults), $select: FIELDS }).toString();
      const { value } = Files.parse(await callGraph(context.fetch, url, { signal: abortSignal }));
      return { files: value.map(summary) };
    },
  }),
  onedrive_read: tool({
    description: `OneDrive: read a plain text file (text, Markdown, CSV, JSON, XML, HTML, code). Word, Excel, and PowerPoint files cannot be read as text: Microsoft Graph converts them only to PDF or images, so the user has to open them from the link. Long files arrive in parts: when truncated is true, call again with offset set to nextOffset. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({ fileId: z.string(), offset: z.int().min(0).optional() }),
    contextSchema: MicrosoftConnection,
    metadata: readMetadata,
    execute: async ({ fileId, offset }, { context, abortSignal }) => {
      const base = `${API}/items/${encodeURIComponent(fileId)}`;
      const found = File.parse(
        await callGraph(context.fetch, `${base}?$select=${FIELDS}`, { signal: abortSignal }),
      );
      const file = summary(found);
      if (found.folder) return { ...file, content: null, note: "This is a folder." };
      if (!isText(found)) {
        return { ...file, content: null, note: "This file is not text; open it from the link." };
      }
      if ((found.size ?? 0) > MAX_READ_BYTES) {
        return { ...file, content: null, note: "This file is too large to read." };
      }
      // The content URL redirects to a download address that needs no token.
      const text = await readGraphText(context.fetch, `${base}/content`, abortSignal);
      return { ...file, ...page(text, offset) };
    },
  }),
  onedrive_share: tool({
    description:
      "OneDrive: create a sharing link for a file or folder. Anonymous links work for anyone who has them; organization links need a work or school account.",
    inputSchema: z.object({
      fileId: z.string(),
      type: z.enum(["view", "edit"]),
      scope: z.enum(["anonymous", "organization"]),
    }),
    contextSchema: MicrosoftConnection,
    execute: async ({ fileId, type, scope }, { context, abortSignal }) => {
      const { link } = Link.parse(
        await callGraph(context.fetch, `${API}/items/${encodeURIComponent(fileId)}/createLink`, {
          method: "POST",
          body: { type, scope },
          signal: abortSignal,
        }),
      );
      return { link: link.webUrl, type, scope };
    },
  }),
  onedrive_trash: tool({
    description: "OneDrive: move a file or folder to the recycle bin.",
    inputSchema: z.object({ fileId: z.string() }),
    contextSchema: MicrosoftConnection,
    execute: async ({ fileId }, { context, abortSignal }) => {
      await callGraph(context.fetch, `${API}/items/${encodeURIComponent(fileId)}`, {
        method: "DELETE",
        signal: abortSignal,
      });
      return { trashed: fileId };
    },
  }),
};

function isText(file: z.infer<typeof File>): boolean {
  // OneDrive reports some text files, such as Markdown, as application/octet-stream.
  return (
    /^text\/|json$|xml$/.test(file.file?.mimeType ?? "") ||
    /\.(txt|md|markdown|csv|tsv|json|xml|html?|log|ya?ml|toml|ini)$/i.test(file.name)
  );
}

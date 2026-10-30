import { tool } from "ai";
import { z } from "zod";
import { callGoogle, GoogleConnection, googleScopes } from "./google";

const API = "https://docs.googleapis.com/v1/documents";
const metadata = { scope: googleScopes.documents };

const Document = z.object({ documentId: z.string(), title: z.string().default("") });

export const googleDocsTools = {
  google_docs_create: tool({
    description:
      "Google Docs: create a document, optionally with text. Read docs with google_drive_read.",
    inputSchema: z.object({ title: z.string(), text: z.string().optional() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ title, text }, { context, abortSignal }) => {
      const document = Document.parse(
        await callGoogle(context.fetch, API, {
          method: "POST",
          body: { title },
          signal: abortSignal,
        }),
      );
      if (text) {
        await update(
          context.fetch,
          document.documentId,
          [{ insertText: { location: { index: 1 }, text } }],
          abortSignal,
        );
      }
      return { ...document, link: linkOf(document.documentId) };
    },
  }),
  google_docs_append: tool({
    description: "Google Docs: add text at the end of a document.",
    inputSchema: z.object({ documentId: z.string(), text: z.string() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ documentId, text }, { context, abortSignal }) => {
      await update(
        context.fetch,
        documentId,
        [{ insertText: { endOfSegmentLocation: {}, text } }],
        abortSignal,
      );
      return { documentId, link: linkOf(documentId) };
    },
  }),
  google_docs_replace_text: tool({
    description: "Google Docs: replace every occurrence of a text in a document.",
    inputSchema: z.object({
      documentId: z.string(),
      find: z.string().min(1),
      replace: z.string(),
      matchCase: z.boolean().default(true),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ documentId, find, replace, matchCase }, { context, abortSignal }) => {
      await update(
        context.fetch,
        documentId,
        [{ replaceAllText: { containsText: { text: find, matchCase }, replaceText: replace } }],
        abortSignal,
      );
      return { documentId, link: linkOf(documentId) };
    },
  }),
};

function update(
  fetch: Parameters<typeof callGoogle>[0],
  documentId: string,
  requests: object[],
  signal: AbortSignal | undefined,
) {
  return callGoogle(fetch, `${API}/${encodeURIComponent(documentId)}:batchUpdate`, {
    method: "POST",
    body: { requests },
    signal,
  });
}

function linkOf(documentId: string): string {
  return `https://docs.google.com/document/d/${documentId}/edit`;
}

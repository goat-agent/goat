import { tool } from "ai";
import { z } from "zod";
import type { Fetch } from "../../integrations/integration";
import { callGraph, MicrosoftConnection, page, untrusted, UNTRUSTED_NOTE } from "./microsoft";

const API = "https://graph.microsoft.com/v1.0/me";
const FIELDS = "id,conversationId,subject,from,toRecipients,receivedDateTime,bodyPreview,isRead";

const Address = z.object({
  emailAddress: z.object({ name: z.string().optional(), address: z.string().optional() }),
});

const Message = z.object({
  id: z.string(),
  conversationId: z.string().optional(),
  subject: z.string().nullish(),
  from: Address.optional(),
  toRecipients: z.array(Address).default([]),
  receivedDateTime: z.string().optional(),
  bodyPreview: z.string().default(""),
  isRead: z.boolean().optional(),
});

const Messages = z.object({ value: z.array(Message).default([]) });

const Body = z.object({ body: z.object({ content: z.string().default("") }) });

const Folders = z.object({
  value: z
    .array(
      z.object({
        id: z.string(),
        displayName: z.string(),
        totalItemCount: z.number().optional(),
        unreadItemCount: z.number().optional(),
      }),
    )
    .default([]),
});

const Draft = z.object({ id: z.string(), webLink: z.string().optional() });

const Moved = z.object({ id: z.string() });

const Outgoing = z.object({
  to: z.array(z.email()).min(1),
  cc: z.array(z.email()).optional(),
  bcc: z.array(z.email()).optional(),
  subject: z.string(),
  body: z.string().describe("Plain text"),
});

export const outlookMailTools = {
  outlook_mail_search: tool({
    description: `Outlook mail: search the user's mail with Microsoft search syntax (from:boss subject:report hasattachments:true received>=2026-10-01), or list the newest messages when no query is given. Returns senders, subjects, dates, and previews. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({
      query: z.string().optional().describe("Search words or syntax such as from:boss@example.com"),
      folder: z
        .string()
        .optional()
        .describe("A folder id from outlook_mail_list_folders, or inbox, drafts, sentitems"),
      maxResults: z.int().min(1).max(25).default(10),
    }),
    contextSchema: MicrosoftConnection,
    execute: async ({ query, folder, maxResults }, { context, abortSignal }) => {
      const scope = folder ? `/mailFolders/${encodeURIComponent(folder)}` : "";
      const url = new URL(`${API}${scope}/messages`);
      url.searchParams.set("$top", String(maxResults));
      url.searchParams.set("$select", FIELDS);
      // Graph does not allow $orderby together with $search; results then come ranked.
      if (query) url.searchParams.set("$search", `"${query.replace(/"/g, " ")}"`);
      else url.searchParams.set("$orderby", "receivedDateTime desc");
      const { value } = Messages.parse(
        await callGraph(context.fetch, url, { signal: abortSignal }),
      );
      return {
        messages: value.map((found) =>
          Object.assign(summary(found), { preview: untrusted(found.bodyPreview) }),
        ),
      };
    },
  }),
  outlook_mail_read: tool({
    description: `Outlook mail: read one message in full as text. Long bodies arrive in parts: when truncated is true, call again with offset set to nextOffset. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({
      messageId: z.string(),
      offset: z.int().min(0).optional(),
    }),
    contextSchema: MicrosoftConnection,
    execute: async ({ messageId, offset }, { context, abortSignal }) => {
      const url = new URL(`${API}/messages/${encodeURIComponent(messageId)}`);
      url.searchParams.set("$select", `${FIELDS},body`);
      const headers = { prefer: 'outlook.body-content-type="text"' };
      const body = await callGraph(context.fetch, url, { headers, signal: abortSignal });
      const message = Message.parse(body);
      return { ...summary(message), ...page(Body.parse(body).body.content, offset) };
    },
  }),
  outlook_mail_send: tool({
    description: "Outlook mail: send a new message from the user's account.",
    inputSchema: Outgoing,
    contextSchema: MicrosoftConnection,
    execute: async (input, { context, abortSignal }) => {
      await callGraph(context.fetch, `${API}/sendMail`, {
        method: "POST",
        body: { message: outgoing(input), saveToSentItems: true },
        signal: abortSignal,
      });
      return { sent: input.to };
    },
  }),
  outlook_mail_reply: tool({
    description: "Outlook mail: reply to a message, to the sender only or to everyone.",
    inputSchema: z.object({
      messageId: z.string(),
      body: z.string().describe("Plain text; the original message is quoted below it"),
      replyAll: z.boolean().default(false),
    }),
    contextSchema: MicrosoftConnection,
    execute: async ({ messageId, body, replyAll }, { context, abortSignal }) => {
      const action = replyAll ? "replyAll" : "reply";
      await callGraph(context.fetch, `${API}/messages/${encodeURIComponent(messageId)}/${action}`, {
        method: "POST",
        body: { comment: body },
        signal: abortSignal,
      });
      return { replied: messageId, replyAll };
    },
  }),
  outlook_mail_create_draft: tool({
    description: "Outlook mail: save a draft the user can review and send from Outlook.",
    inputSchema: Outgoing,
    contextSchema: MicrosoftConnection,
    execute: async (input, { context, abortSignal }) => {
      const draft = Draft.parse(
        await callGraph(context.fetch, `${API}/messages`, {
          method: "POST",
          body: outgoing(input),
          signal: abortSignal,
        }),
      );
      return { draft: draft.id, link: draft.webLink };
    },
  }),
  outlook_mail_list_folders: tool({
    description: "Outlook mail: list the user's top-level mail folders and their ids.",
    inputSchema: z.object({}),
    contextSchema: MicrosoftConnection,
    execute: async (_, { context, abortSignal }) =>
      Folders.parse(
        await callGraph(context.fetch, `${API}/mailFolders?$top=100`, { signal: abortSignal }),
      ),
  }),
  outlook_mail_move: tool({
    description:
      "Outlook mail: move a message to a folder. Use a folder id from outlook_mail_list_folders, or archive, inbox, junkemail. The message gets a new id.",
    inputSchema: z.object({ messageId: z.string(), folder: z.string() }),
    contextSchema: MicrosoftConnection,
    execute: async ({ messageId, folder }, { context, abortSignal }) => {
      const moved = await move(context.fetch, messageId, folder, abortSignal);
      return { moved: moved.id };
    },
  }),
  outlook_mail_trash: tool({
    description: "Outlook mail: move a message to Deleted Items.",
    inputSchema: z.object({ messageId: z.string() }),
    contextSchema: MicrosoftConnection,
    execute: async ({ messageId }, { context, abortSignal }) => {
      await move(context.fetch, messageId, "deleteditems", abortSignal);
      return { trashed: messageId };
    },
  }),
};

async function move(
  fetch: Fetch,
  messageId: string,
  folder: string,
  signal: AbortSignal | undefined,
) {
  return Moved.parse(
    await callGraph(fetch, `${API}/messages/${encodeURIComponent(messageId)}/move`, {
      method: "POST",
      body: { destinationId: folder },
      signal,
    }),
  );
}

function summary(message: z.infer<typeof Message>) {
  return {
    id: message.id,
    conversationId: message.conversationId,
    from: message.from?.emailAddress.address,
    fromName: message.from?.emailAddress.name,
    to: message.toRecipients.flatMap((to) => to.emailAddress.address ?? []),
    subject: untrusted(message.subject ?? ""),
    date: message.receivedDateTime,
    isRead: message.isRead,
  };
}

function recipients(addresses: string[] | undefined) {
  return addresses?.map((address) => ({ emailAddress: { address } })) ?? [];
}

function outgoing(input: z.infer<typeof Outgoing>) {
  return {
    subject: input.subject,
    body: { contentType: "Text", content: input.body },
    toRecipients: recipients(input.to),
    ccRecipients: recipients(input.cc),
    bccRecipients: recipients(input.bcc),
  };
}

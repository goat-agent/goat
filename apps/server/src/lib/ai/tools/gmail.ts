import { tool } from "ai";
import { z } from "zod";
import type { Fetch } from "../../integrations/integration";
import {
  callGoogle,
  GoogleConnection,
  googleScopes,
  page,
  untrusted,
  UNTRUSTED_NOTE,
} from "./google";

const API = "https://gmail.googleapis.com/gmail/v1/users/me";
const metadata = { scope: googleScopes.gmail };

const Header = z.object({ name: z.string(), value: z.string() });

type Part = {
  mimeType?: string | undefined;
  headers?: z.infer<typeof Header>[] | undefined;
  body?: { data?: string | undefined } | undefined;
  parts?: Part[] | undefined;
};

const Part: z.ZodType<Part> = z.lazy(() =>
  z.object({
    mimeType: z.string().optional(),
    headers: z.array(Header).optional(),
    body: z.object({ data: z.string().optional() }).optional(),
    parts: z.array(Part).optional(),
  }),
);

const Message = z.object({
  id: z.string(),
  threadId: z.string(),
  labelIds: z.array(z.string()).default([]),
  snippet: z.string().default(""),
  payload: Part.optional(),
});

const MessageList = z.object({
  messages: z.array(z.object({ id: z.string() })).default([]),
});

const Labels = z.object({
  labels: z.array(z.object({ id: z.string(), name: z.string() })).default([]),
});

const Sent = z.object({ id: z.string(), threadId: z.string() });

const Draft = z.object({ id: z.string(), message: Sent });

const Outgoing = z.object({
  to: z.array(z.email()).default([]),
  cc: z.array(z.email()).optional(),
  bcc: z.array(z.email()).optional(),
  subject: z.string().optional().describe("Defaults to the original subject when replying"),
  body: z.string().describe("Plain text"),
  replyToMessageId: z
    .string()
    .optional()
    .describe("Send as a reply in that message's thread; add the sender to `to` yourself"),
});

export const gmailTools = {
  google_gmail_search: tool({
    description: `Gmail: search the user's mail with Gmail search syntax (from:, to:, subject:, is:unread, newer_than:2d). Returns senders, subjects, dates, and snippets. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({
      query: z.string().describe("Gmail search, such as from:boss@example.com is:unread"),
      maxResults: z.int().min(1).max(25).default(10),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ query, maxResults }, { context, abortSignal }) => {
      const url = new URL(`${API}/messages`);
      url.search = new URLSearchParams({ q: query, maxResults: String(maxResults) }).toString();
      const { messages } = MessageList.parse(
        await callGoogle(context.fetch, url, { signal: abortSignal }),
      );
      const found = await Promise.all(
        messages.map(({ id }) => metadataOf(context.fetch, id, abortSignal)),
      );
      return {
        messages: found.map((message) =>
          Object.assign(summary(message), { snippet: untrusted(message.snippet) }),
        ),
      };
    },
  }),
  google_gmail_read: tool({
    description: `Gmail: read one message in full. Long bodies arrive in parts: when truncated is true, call again with offset set to nextOffset. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({
      messageId: z.string(),
      offset: z.int().min(0).optional(),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ messageId, offset }, { context, abortSignal }) => {
      const message = Message.parse(
        await callGoogle(context.fetch, `${API}/messages/${encodeURIComponent(messageId)}`, {
          signal: abortSignal,
        }),
      );
      return { ...summary(message), ...page(bodyText(message.payload), offset) };
    },
  }),
  google_gmail_send: tool({
    description: "Gmail: send a message from the user's account, or a reply in a thread.",
    inputSchema: Outgoing,
    contextSchema: GoogleConnection,
    metadata,
    execute: async (input, { context, abortSignal }) => {
      const message = await compose(context.fetch, input, abortSignal);
      const sent = Sent.parse(
        await callGoogle(context.fetch, `${API}/messages/send`, {
          method: "POST",
          body: message,
          signal: abortSignal,
        }),
      );
      return { sent: sent.id, threadId: sent.threadId };
    },
  }),
  google_gmail_create_draft: tool({
    description: "Gmail: save a draft the user can review and send from Gmail.",
    inputSchema: Outgoing,
    contextSchema: GoogleConnection,
    metadata,
    execute: async (input, { context, abortSignal }) => {
      const message = await compose(context.fetch, input, abortSignal);
      const draft = Draft.parse(
        await callGoogle(context.fetch, `${API}/drafts`, {
          method: "POST",
          body: { message },
          signal: abortSignal,
        }),
      );
      return { draft: draft.id, threadId: draft.message.threadId };
    },
  }),
  google_gmail_list_labels: tool({
    description: "Gmail: list the user's labels and their ids.",
    inputSchema: z.object({}),
    contextSchema: GoogleConnection,
    metadata,
    execute: async (_, { context, abortSignal }) =>
      Labels.parse(await callGoogle(context.fetch, `${API}/labels`, { signal: abortSignal })),
  }),
  google_gmail_change_labels: tool({
    description:
      "Gmail: add or remove labels on a message. Remove INBOX to archive, UNREAD to mark read; add STARRED to star. Other labels by id from google_gmail_list_labels.",
    inputSchema: z.object({
      messageId: z.string(),
      add: z.array(z.string()).default([]),
      remove: z.array(z.string()).default([]),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ messageId, add, remove }, { context, abortSignal }) => {
      const url = `${API}/messages/${encodeURIComponent(messageId)}/modify`;
      const message = Message.parse(
        await callGoogle(context.fetch, url, {
          method: "POST",
          body: { addLabelIds: add, removeLabelIds: remove },
          signal: abortSignal,
        }),
      );
      return { id: message.id, labels: message.labelIds };
    },
  }),
  google_gmail_trash: tool({
    description: "Gmail: move a message to the trash.",
    inputSchema: z.object({ messageId: z.string() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ messageId }, { context, abortSignal }) => {
      const url = `${API}/messages/${encodeURIComponent(messageId)}/trash`;
      await callGoogle(context.fetch, url, { method: "POST", signal: abortSignal });
      return { trashed: messageId };
    },
  }),
};

async function metadataOf(fetch: Fetch, id: string, signal: AbortSignal | undefined) {
  const url = new URL(`${API}/messages/${encodeURIComponent(id)}`);
  url.searchParams.set("format", "metadata");
  for (const name of ["From", "To", "Cc", "Subject", "Date", "Message-ID", "References"]) {
    url.searchParams.append("metadataHeaders", name);
  }
  return Message.parse(await callGoogle(fetch, url, { signal }));
}

function summary(message: z.infer<typeof Message>) {
  const header = headerOf(message.payload);
  return {
    id: message.id,
    threadId: message.threadId,
    from: header("From"),
    to: header("To"),
    cc: header("Cc"),
    subject: header("Subject"),
    date: header("Date"),
    labels: message.labelIds,
  };
}

function headerOf(payload: Part | undefined) {
  return (name: string) =>
    payload?.headers?.find((header) => header.name.toLowerCase() === name.toLowerCase())?.value;
}

function bodyText(payload: Part | undefined): string {
  if (!payload) return "";
  const plain = findPart(payload, "text/plain");
  if (plain) return plain;
  const html = findPart(payload, "text/html");
  return html ? textFromHtml(html) : "";
}

function findPart(part: Part, mimeType: string): string | undefined {
  if (part.mimeType === mimeType && part.body?.data) return decode(part.body.data);
  for (const child of part.parts ?? []) {
    const found = findPart(child, mimeType);
    if (found) return found;
  }
  return undefined;
}

function textFromHtml(html: string): string {
  return html
    .replace(/<(script|style)[\s\S]*?<\/\1>/gi, " ")
    .replace(/<br\s*\/?>|<\/(p|div|li|tr|h\d)>/gi, "\n")
    .replace(/<[^>]+>/g, " ")
    .replace(/&nbsp;/g, " ")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/[ \t]+/g, " ")
    .replace(/\n\s*\n+/g, "\n\n")
    .trim();
}

async function compose(
  fetch: Fetch,
  input: z.infer<typeof Outgoing>,
  signal: AbortSignal | undefined,
): Promise<{ raw: string; threadId?: string }> {
  const original = input.replyToMessageId
    ? await metadataOf(fetch, input.replyToMessageId, signal)
    : undefined;
  const header = headerOf(original?.payload);
  const originalSubject = header("Subject") ?? "";
  const subject =
    input.subject ??
    (original ? (/^re:/i.test(originalSubject) ? originalSubject : `Re: ${originalSubject}`) : "");
  const messageId = header("Message-ID");
  const references = [header("References"), messageId].filter(Boolean).join(" ");

  const lines = [
    `To: ${input.to.join(", ")}`,
    ...(input.cc?.length ? [`Cc: ${input.cc.join(", ")}`] : []),
    ...(input.bcc?.length ? [`Bcc: ${input.bcc.join(", ")}`] : []),
    `Subject: =?UTF-8?B?${Buffer.from(subject).toString("base64")}?=`,
    ...(messageId ? [`In-Reply-To: ${messageId}`, `References: ${references}`] : []),
    "MIME-Version: 1.0",
    'Content-Type: text/plain; charset="UTF-8"',
    "Content-Transfer-Encoding: base64",
    "",
    ...(Buffer.from(input.body)
      .toString("base64")
      .match(/.{1,76}/g) ?? []),
  ];
  return {
    raw: Buffer.from(lines.join("\r\n")).toString("base64url"),
    ...(original ? { threadId: original.threadId } : {}),
  };
}

function decode(data: string): string {
  return Buffer.from(data, "base64url").toString("utf8");
}

import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import { APICallError, type AssistantModelMessage, type ModelMessage } from "ai";
import { and, asc, desc, eq, gt, lt } from "drizzle-orm";
import { bodyLimit } from "hono/body-limit";
import { HTTPException } from "hono/http-exception";
import { streamSSE } from "hono/streaming";
import { db } from "../db";
import { agents, messages } from "../db/schema/agents";
import { type Agent as AgentRow, createAgent, enqueue } from "../lib/agent";
import { type TranscriptionHints, transcribe } from "../lib/ai/providers";
import { stepOf } from "../lib/ai/tools/computer";
import { startCall } from "../lib/calls/index";
import { recentLines, textOf } from "../lib/conversation";
import { subscribe } from "../lib/events";
import * as files from "../lib/files";
import { localeOf } from "../lib/i18n";
import { listConnections, matchIntegration } from "../lib/integrations/connections";
import { integrations } from "../lib/integrations/index";
import { logger } from "../lib/logger";
import { body, error, invalidRequest, json, openEnum } from "../lib/openapi";
import { session, type SessionEnv } from "../middlewares/session";

const KEEPALIVE_MS = 20_000;

const Agent = z
  .object({ id: z.uuid(), name: z.string(), instructions: z.string(), createdAt: z.iso.datetime() })
  .openapi("Agent");

const ConnectRequest = z
  .object({ integrationId: z.string(), name: z.string() })
  .openapi("ConnectRequest");

const File = z
  .object({
    id: z.uuid(),
    name: z.string(),
    mediaType: z.string().describe("Pick how to show the file from this; any type may appear"),
  })
  .openapi("File");

const Message = z
  .object({
    id: z.int(),
    role: openEnum(["user", "assistant"]),
    text: z.string(),
    files: z.array(File).optional().describe("Files the user sent with the message, in order"),
    connect: z
      .array(ConnectRequest)
      .optional()
      .describe("Services the agent asked the user to connect; show a button for each"),
    steps: z
      .array(z.string())
      .optional()
      .describe("What the agent did on the user's computers, one short line each, in order"),
    createdAt: z.iso.datetime(),
  })
  .openapi("Message");

const ConnectInput = z.object({ integration: z.string() });

const AgentEvent = z
  .object({
    type: openEnum(["message", "status"]),
    message: Message.optional(),
    status: openEnum(["running", "idle"]).optional(),
  })
  .openapi("AgentEvent");

const AgentParams = z.object({ id: z.uuid() });

const FileParams = z.object({ id: z.uuid(), fileId: z.uuid() });

// Multipart framing adds a little on top of the file itself.
const UPLOAD_BODY_LIMIT = files.maxUploadBytes + 64 * 1024;

// OpenAI's limit for one transcription request.
const MAX_AUDIO_BYTES = 25 * 1024 * 1024;
const AUDIO_BODY_LIMIT = MAX_AUDIO_BYTES + 64 * 1024;

const MAX_SDP_CHARS = 20_000;

const RECENT_MESSAGES = 6;
const MAX_CONTEXT_CHARS = 1_000;

const log = logger.child({ module: "agents" });

type MessageRow = { id: number; message: ModelMessage; hidden: boolean; createdAt: Date };

function publicAgent(agent: AgentRow): z.infer<typeof Agent> {
  return {
    id: agent.id,
    name: agent.name,
    instructions: agent.instructions,
    createdAt: agent.createdAt.toISOString(),
  };
}

function publicMessage({
  id,
  message,
  hidden,
  createdAt,
}: MessageRow): z.infer<typeof Message> | undefined {
  if (hidden) return undefined;
  const text = textOf(message);
  if (message.role === "user") {
    const attached = files.attachments(message.content);
    if (!text && attached.length === 0) return undefined;
    return {
      id,
      role: message.role,
      text,
      ...(attached.length > 0 ? { files: attached } : {}),
      createdAt: createdAt.toISOString(),
    };
  }
  if (message.role !== "assistant") return undefined;
  const connect = connectRequests(message);
  const steps = stepsOf(message);
  if (!text && connect.length === 0 && steps.length === 0) return undefined;
  return {
    id,
    role: message.role,
    text,
    ...(connect.length > 0 ? { connect } : {}),
    ...(steps.length > 0 ? { steps } : {}),
    createdAt: createdAt.toISOString(),
  };
}

function connectRequests(message: AssistantModelMessage): z.infer<typeof ConnectRequest>[] {
  if (typeof message.content === "string") return [];
  return message.content.flatMap((part) => {
    if (part.type !== "tool-call" || part.toolName !== "connect") return [];
    const input = ConnectInput.safeParse(part.input);
    const integrationId = input.success ? matchIntegration(input.data.integration) : undefined;
    const integration = integrationId ? integrations[integrationId] : undefined;
    return integrationId && integration ? [{ integrationId, name: integration.name }] : [];
  });
}

function stepsOf(message: AssistantModelMessage): string[] {
  if (typeof message.content === "string") return [];
  return message.content.flatMap((part) => {
    const step = part.type === "tool-call" ? stepOf(part.toolName, part.input) : undefined;
    return step ? [step] : [];
  });
}

async function ownAgent(userId: string, agentId: string): Promise<void> {
  const [agent] = await db
    .select({ id: agents.id })
    .from(agents)
    .where(and(eq(agents.id, agentId), eq(agents.userId, userId)));
  if (!agent) throw new HTTPException(404, { message: "agent not found" });
}

/** Recordings from Apple's recorder (MP4/M4A) and from browsers (WebM). */
function isRecording(bytes: Uint8Array): boolean {
  const isMp4 = new TextDecoder().decode(bytes.subarray(4, 8)) === "ftyp";
  const isWebm = [0x1a, 0x45, 0xdf, 0xa3].every((byte, index) => bytes[index] === byte);
  return isMp4 || isWebm;
}

async function transcriptionHints(
  user: { id: string; name: string },
  agentId: string,
): Promise<TranscriptionHints> {
  const [[agent], recent, connected] = await Promise.all([
    db.select({ name: agents.name }).from(agents).where(eq(agents.id, agentId)),
    recentLines(agentId, RECENT_MESSAGES),
    listConnections(user.id),
  ]);
  const context = recent
    .map(({ text }) => text)
    .join("\n")
    .slice(-MAX_CONTEXT_CHARS);
  const names = connected.flatMap((connection) => {
    const integration = integrations[connection.integrationId];
    return integration ? [integration.name, connection.accountName] : [];
  });
  // The API refuses the whole request over one keyword with these characters.
  const keywords = [user.name, agent?.name ?? "", ...names]
    .map((keyword) => keyword.trim())
    .filter((keyword) => keyword && !/[<>\r\n]/.test(keyword));
  return { context, keywords: [...new Set(keywords)] };
}

async function statusOf(agentId: string): Promise<"running" | "idle"> {
  const [agent] = await db
    .select({ runOwner: agents.runOwner })
    .from(agents)
    .where(eq(agents.id, agentId));
  return agent?.runOwner ? "running" : "idle";
}

const columns = {
  id: messages.id,
  message: messages.message,
  hidden: messages.hidden,
  createdAt: messages.createdAt,
};

const listAgents = createRoute({
  operationId: "listAgents",
  method: "get",
  path: "/",
  security: [{ bearer: [] }],
  responses: {
    200: json(z.object({ agents: z.array(Agent) }), "The user's agents, oldest first"),
    401: error("Not signed in"),
  },
});

const createAgentRoute = createRoute({
  operationId: "createAgent",
  method: "post",
  path: "/",
  security: [{ bearer: [] }],
  request: {
    body: body(
      z.object({
        name: z.string().trim().min(1).max(100).default("Goat"),
        instructions: z.string().trim().max(10_000).default(""),
      }),
      "The agent funnel",
    ),
  },
  responses: {
    201: json(z.object({ agent: Agent }), "The new agent, with no messages yet"),
    400: error("Invalid request"),
    401: error("Not signed in"),
    409: error("The user has no name yet"),
  },
});

const listMessages = createRoute({
  operationId: "listMessages",
  method: "get",
  path: "/{id}/messages",
  security: [{ bearer: [] }],
  request: {
    params: AgentParams,
    query: z.object({
      before: z.coerce.number().int().positive().optional(),
      limit: z.coerce.number().int().min(1).max(100).default(50),
    }),
  },
  responses: {
    200: json(z.object({ messages: z.array(Message) }), "A page of messages, oldest first"),
    401: error("Not signed in"),
    404: error("No such agent"),
  },
});

const sendMessage = createRoute({
  operationId: "sendMessage",
  method: "post",
  path: "/{id}/messages",
  security: [{ bearer: [] }],
  request: {
    params: AgentParams,
    body: body(
      z
        .object({
          text: z.string().trim().max(20_000).default(""),
          files: z
            .array(z.uuid())
            .max(10)
            .default([])
            .describe("Ids from uploadFile, in the order to show them"),
        })
        .refine(({ text, files: ids }) => text.length > 0 || ids.length > 0),
      "The user's message: text, files, or both",
    ),
  },
  responses: {
    202: { description: "Queued; the reply arrives on the event stream" },
    400: error("Invalid request, or a file that is not an upload to this agent"),
    401: error("Not signed in"),
    404: error("No such agent"),
  },
});

const uploadFile = createRoute({
  operationId: "uploadFile",
  method: "post",
  path: "/{id}/files",
  security: [{ bearer: [] }],
  middleware: [bodyLimit({ maxSize: UPLOAD_BODY_LIMIT })],
  request: {
    params: AgentParams,
    body: {
      content: {
        "multipart/form-data": {
          schema: z.object({ file: z.file().openapi({ type: "string", format: "binary" }) }),
          encoding: { file: { contentType: "application/octet-stream" } },
        },
      },
      required: true,
      description: "The file and its name; the server tells its type from the bytes and the name",
    },
  },
  responses: {
    201: json(
      z.object({ file: File }),
      "Stored; send its id with a message within a day or it is deleted",
    ),
    400: error("Invalid request, an unsupported type, or a file too large"),
    401: error("Not signed in"),
    404: error("No such agent"),
    413: error("Body too large"),
  },
});

const downloadFile = createRoute({
  operationId: "downloadFile",
  method: "get",
  path: "/{id}/files/{fileId}",
  security: [{ bearer: [] }],
  request: { params: FileParams },
  responses: {
    200: {
      content: { "*/*": { schema: z.string().openapi({ format: "binary" }) } },
      description: "The file; its bytes never change, so cache it by id",
    },
    401: error("Not signed in"),
    404: error("No such agent or file"),
  },
});

const transcribeAudioRoute = createRoute({
  operationId: "transcribeAudio",
  method: "post",
  path: "/{id}/transcriptions",
  security: [{ bearer: [] }],
  middleware: [bodyLimit({ maxSize: AUDIO_BODY_LIMIT })],
  request: {
    params: AgentParams,
    body: {
      content: {
        "multipart/form-data": {
          schema: z.object({ audio: z.file().openapi({ type: "string", format: "binary" }) }),
          encoding: { audio: { contentType: "application/octet-stream" } },
        },
      },
      required: true,
      description: "A recording, M4A or WebM, up to 25 MB; the server keeps none of it",
    },
  },
  responses: {
    200: json(z.object({ text: z.string() }), "What was said; empty when nothing was"),
    400: error("Invalid request, or not an M4A or WebM recording"),
    401: error("Not signed in"),
    404: error("No such agent"),
    413: error("Body too large"),
    502: error("The recording could not be transcribed"),
  },
});

const startCallRoute = createRoute({
  operationId: "startCall",
  method: "post",
  path: "/{id}/calls",
  security: [{ bearer: [] }],
  request: {
    params: AgentParams,
    body: body(
      z.object({ sdp: z.string().min(1).max(MAX_SDP_CHARS).describe("The device's WebRTC offer") }),
      "A voice call with the agent; requests and replies land in the conversation",
    ),
  },
  responses: {
    201: json(
      z.object({ sdp: z.string().describe("The WebRTC answer") }),
      "The call is ready; audio flows once the answer is set. Close the connection to hang up",
    ),
    400: error("Invalid request"),
    401: error("Not signed in"),
    404: error("No such agent"),
    502: error("The call could not be started"),
  },
});

const streamEvents = createRoute({
  operationId: "streamEvents",
  method: "get",
  path: "/{id}/events",
  security: [{ bearer: [] }],
  request: {
    params: AgentParams,
    headers: z.object({ "last-event-id": z.string().optional() }),
  },
  responses: {
    200: {
      content: { "text/event-stream": { schema: AgentEvent } },
      description: "Messages after Last-Event-ID, then new messages and status changes",
    },
    401: error("Not signed in"),
    404: error("No such agent"),
  },
});

const app = new OpenAPIHono<SessionEnv>({ defaultHook: invalidRequest });
app.use(session);

export default app
  .openapi(listAgents, async (c) => {
    const rows = await db
      .select()
      .from(agents)
      .where(eq(agents.userId, c.var.user.id))
      .orderBy(asc(agents.createdAt));
    return c.json({ agents: rows.map(publicAgent) }, 200);
  })
  .openapi(createAgentRoute, async (c) => {
    const user = c.var.user;
    if (!user.name.trim()) return c.json({ error: "user name is required" }, 409);
    const agent = await createAgent(user.id, c.req.valid("json"));
    return c.json({ agent: publicAgent(agent) }, 201);
  })
  .openapi(listMessages, async (c) => {
    const { id } = c.req.valid("param");
    const { before, limit } = c.req.valid("query");
    await ownAgent(c.var.user.id, id);

    const rows = await db
      .select(columns)
      .from(messages)
      .where(and(eq(messages.agentId, id), before ? lt(messages.id, before) : undefined))
      .orderBy(desc(messages.id))
      .limit(limit);
    const page = rows.toReversed().flatMap((row) => publicMessage(row) ?? []);
    return c.json({ messages: page }, 200);
  })
  .openapi(sendMessage, async (c) => {
    const { id } = c.req.valid("param");
    await ownAgent(c.var.user.id, id);
    const { text, files: fileIds } = c.req.valid("json");
    try {
      await enqueue(id, { text, fileIds }, c.var.user.timeZone ?? undefined);
    } catch (cause) {
      if (cause instanceof files.FileNotFound) return c.json({ error: cause.message }, 400);
      throw cause;
    }
    return c.body(null, 202);
  })
  .openapi(uploadFile, async (c) => {
    const { id } = c.req.valid("param");
    await ownAgent(c.var.user.id, id);
    let stored: files.StoredFile;
    try {
      stored = await files.upload(id, c.req.valid("form").file);
    } catch (cause) {
      if (cause instanceof files.FileRejected) return c.json({ error: cause.message }, 400);
      throw cause;
    }
    const { name, mediaType } = stored;
    return c.json({ file: { id: stored.id, name, mediaType } }, 201);
  })
  .openapi(downloadFile, async (c) => {
    const { id, fileId } = c.req.valid("param");
    await ownAgent(c.var.user.id, id);
    const file = await files.find(id, fileId);
    if (!file) return c.json({ error: "file not found" }, 404);
    return c.body(files.open(file.id).stream(), 200, {
      "content-type": file.mediaType,
      "content-disposition": `inline; filename*=UTF-8''${encodeURIComponent(file.name)}`,
      "cache-control": "private, max-age=31536000, immutable",
    });
  })
  .openapi(transcribeAudioRoute, async (c) => {
    const { id } = c.req.valid("param");
    await ownAgent(c.var.user.id, id);
    const { audio } = c.req.valid("form");
    if (audio.size > MAX_AUDIO_BYTES) return c.json({ error: "recording too large" }, 400);
    const bytes = new Uint8Array(await audio.arrayBuffer());
    if (!isRecording(bytes)) return c.json({ error: "not an M4A or WebM recording" }, 400);

    const hints = await transcriptionHints(c.var.user, id);
    let text: string;
    try {
      text = await transcribe(bytes, hints);
    } catch (cause) {
      if (!APICallError.isInstance(cause)) throw cause;
      // The error carries the request, whose prompt holds message text, so log only the status.
      log.warn({ agentId: id, status: cause.statusCode }, "transcription failed");
      return c.json({ error: "transcription failed" }, 502);
    }
    return c.json({ text }, 200);
  })
  .openapi(startCallRoute, async (c) => {
    const { id } = c.req.valid("param");
    const [agent] = await db
      .select({ id: agents.id, name: agents.name, instructions: agents.instructions })
      .from(agents)
      .where(and(eq(agents.id, id), eq(agents.userId, c.var.user.id)));
    if (!agent) return c.json({ error: "agent not found" }, 404);

    const { sdp } = c.req.valid("json");
    const locale = localeOf(c.req.raw.headers);
    try {
      const { answer } = await startCall(c.var.user, agent, sdp, locale);
      return c.json({ sdp: answer }, 201);
    } catch (cause) {
      log.warn({ err: cause, agentId: id }, "call failed to start");
      return c.json({ error: "call failed to start" }, 502);
    }
  })
  .openapi(streamEvents, async (c) => {
    const { id } = c.req.valid("param");
    await ownAgent(c.var.user.id, id);
    const lastEventId = Number(c.req.valid("header")["last-event-id"]);

    return streamSSE(c, async (stream) => {
      let cursor = Number.isSafeInteger(lastEventId) && lastEventId > 0 ? lastEventId : 0;
      let status: string | undefined;
      let changed = true;
      let wake: (() => void) | undefined;

      const unsubscribe = await subscribe(id, () => {
        changed = true;
        wake?.();
      });
      stream.onAbort(() => wake?.());

      const waitForChange = () =>
        new Promise<boolean>((resolve) => {
          if (changed) return resolve(true);
          const timer = setTimeout(() => resolve(false), KEEPALIVE_MS);
          wake = () => {
            clearTimeout(timer);
            resolve(true);
          };
        });

      const send = (event: z.infer<typeof AgentEvent>, eventId?: number) =>
        stream.writeSSE({
          event: event.type,
          data: JSON.stringify(event),
          ...(eventId === undefined ? {} : { id: String(eventId) }),
        });

      const flush = async () => {
        changed = false;
        const rows = await db
          .select(columns)
          .from(messages)
          .where(and(eq(messages.agentId, id), gt(messages.id, cursor)))
          .orderBy(asc(messages.id));
        for (const row of rows) {
          cursor = row.id;
          const message = publicMessage(row);
          if (message) await send({ type: "message", message }, row.id);
        }

        const current = await statusOf(id);
        if (current !== status) {
          status = current;
          await send({ type: "status", status: current });
        }
      };

      try {
        while (!stream.aborted) {
          if (await waitForChange()) await flush();
          else await stream.write(": keepalive\n\n");
        }
      } finally {
        unsubscribe();
      }
    });
  });

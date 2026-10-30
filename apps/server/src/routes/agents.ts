import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import type { ModelMessage } from "ai";
import { and, asc, desc, eq, gt, lt } from "drizzle-orm";
import { HTTPException } from "hono/http-exception";
import { streamSSE } from "hono/streaming";
import { db } from "../db";
import { agents, messages } from "../db/schema/agents";
import { type Agent as AgentRow, createAgent, enqueue } from "../lib/agent";
import { subscribe } from "../lib/events";
import { body, error, invalidRequest, json, openEnum } from "../lib/openapi";
import { session, type SessionEnv } from "../middlewares/session";

const KEEPALIVE_MS = 20_000;

const Agent = z
  .object({ id: z.uuid(), name: z.string(), instructions: z.string(), createdAt: z.iso.datetime() })
  .openapi("Agent");

const Message = z
  .object({
    id: z.int(),
    role: openEnum(["user", "assistant"]),
    text: z.string(),
    createdAt: z.iso.datetime(),
  })
  .openapi("Message");

const AgentEvent = z
  .object({
    type: openEnum(["message", "status"]),
    message: Message.optional(),
    status: openEnum(["running", "idle"]).optional(),
  })
  .openapi("AgentEvent");

const AgentParams = z.object({ id: z.uuid() });

type MessageRow = { id: number; message: ModelMessage; createdAt: Date };

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
  createdAt,
}: MessageRow): z.infer<typeof Message> | undefined {
  if (message.role !== "user" && message.role !== "assistant") return undefined;
  const texts =
    typeof message.content === "string"
      ? [message.content]
      : message.content.flatMap((part) => (part.type === "text" ? [part.text] : []));
  const text = message.role === "user" ? texts.at(-1) : texts.join("");
  return text ? { id, role: message.role, text, createdAt: createdAt.toISOString() } : undefined;
}

async function ownAgent(userId: string, agentId: string): Promise<void> {
  const [agent] = await db
    .select({ id: agents.id })
    .from(agents)
    .where(and(eq(agents.id, agentId), eq(agents.userId, userId)));
  if (!agent) throw new HTTPException(404, { message: "agent not found" });
}

async function statusOf(agentId: string): Promise<"running" | "idle"> {
  const [agent] = await db
    .select({ runOwner: agents.runOwner })
    .from(agents)
    .where(eq(agents.id, agentId));
  return agent?.runOwner ? "running" : "idle";
}

const columns = { id: messages.id, message: messages.message, createdAt: messages.createdAt };

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
    201: json(z.object({ agent: Agent }), "The new agent; its greeting is already in its messages"),
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
    body: body(z.object({ text: z.string().trim().min(1).max(20_000) }), "The user's message"),
  },
  responses: {
    202: { description: "Queued; the reply arrives on the event stream" },
    400: error("Invalid request"),
    401: error("Not signed in"),
    404: error("No such agent"),
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
    const agent = await createAgent(user, c.req.valid("json"));
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
    await enqueue(id, c.req.valid("json").text, c.var.user.timeZone ?? undefined);
    return c.body(null, 202);
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

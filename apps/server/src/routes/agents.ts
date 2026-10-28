import { zValidator } from "@hono/zod-validator";
import type { ModelMessage } from "ai";
import { and, asc, desc, eq, gt, lt } from "drizzle-orm";
import { Hono } from "hono";
import { HTTPException } from "hono/http-exception";
import { streamSSE } from "hono/streaming";
import { z } from "zod";
import { db } from "../db";
import { agents, messages } from "../db/schema/agents";
import { type Agent, createAgent, enqueue } from "../lib/agent";
import { subscribe } from "../lib/events";
import { session, type SessionEnv } from "../middlewares/session";

const KEEPALIVE_MS = 20_000;

const agentParam = z.object({ id: z.uuid() });

type MessageRow = { id: number; message: ModelMessage; createdAt: Date };

function publicAgent(agent: Agent) {
  return {
    id: agent.id,
    name: agent.name,
    instructions: agent.instructions,
    createdAt: agent.createdAt,
  };
}

function publicMessage({ id, message, createdAt }: MessageRow) {
  if (message.role !== "user" && message.role !== "assistant") return undefined;
  const text =
    typeof message.content === "string"
      ? message.content
      : message.content.flatMap((part) => (part.type === "text" ? [part.text] : [])).join("");
  return text ? { id, role: message.role, text, createdAt } : undefined;
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

export default new Hono<SessionEnv>()
  .use(session)
  .get("/", async (c) => {
    const rows = await db
      .select()
      .from(agents)
      .where(eq(agents.userId, c.var.user.id))
      .orderBy(asc(agents.createdAt));
    return c.json({ agents: rows.map(publicAgent) });
  })
  .post(
    "/",
    zValidator(
      "json",
      z.object({
        name: z.string().trim().min(1).max(100).default("Goat"),
        instructions: z.string().trim().max(10_000).default(""),
      }),
    ),
    async (c) => {
      const user = c.var.user;
      if (!user.name.trim()) throw new HTTPException(409, { message: "user name is required" });
      const agent = await createAgent(user, c.req.valid("json"));
      return c.json({ agent: publicAgent(agent) }, 201);
    },
  )
  .get(
    "/:id/messages",
    zValidator("param", agentParam),
    zValidator(
      "query",
      z.object({
        before: z.coerce.number().int().positive().optional(),
        limit: z.coerce.number().int().min(1).max(100).default(50),
      }),
    ),
    async (c) => {
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
      return c.json({ messages: page });
    },
  )
  .post(
    "/:id/messages",
    zValidator("param", agentParam),
    zValidator("json", z.object({ text: z.string().trim().min(1).max(20_000) })),
    async (c) => {
      const { id } = c.req.valid("param");
      await ownAgent(c.var.user.id, id);
      await enqueue(id, c.req.valid("json").text);
      return c.body(null, 202);
    },
  )
  .get("/:id/events", zValidator("param", agentParam), async (c) => {
    const { id } = c.req.valid("param");
    await ownAgent(c.var.user.id, id);
    const lastEventId = Number(c.req.header("Last-Event-ID"));

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
          if (message) {
            await stream.writeSSE({
              id: String(row.id),
              event: "message",
              data: JSON.stringify(message),
            });
          }
        }

        const current = await statusOf(id);
        if (current !== status) {
          status = current;
          await stream.writeSSE({ event: "status", data: current });
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

import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import { and, eq } from "drizzle-orm";
import { db } from "../db";
import { agents } from "../db/schema/agents";
import { devices } from "../db/schema/devices";
import { env } from "../env";
import { resumeAfterConnect } from "../lib/agent";
import { finishConnecting, returnToClient, startConnecting } from "../lib/integrations/authorize";
import { listConnections } from "../lib/integrations/connections";
import { integrations } from "../lib/integrations/index";
import { body, error, invalidRequest, json } from "../lib/openapi";
import { logger } from "../lib/logger";
import { session, type SessionEnv } from "../middlewares/session";

const log = logger.child({ module: "integrations" });

const ICONS = new URL("../../assets/integrations/", import.meta.url);

const Connection = z
  .object({ id: z.uuid(), accountName: z.string(), needsReconnect: z.boolean() })
  .openapi("Connection");

const Integration = z
  .object({
    id: z.string(),
    name: z.string(),
    iconUrl: z.url(),
    connections: z.array(Connection),
  })
  .openapi("Integration");

const IntegrationParams = z.object({ id: z.string() });

const listIntegrations = createRoute({
  operationId: "listIntegrations",
  method: "get",
  path: "/",
  security: [{ bearer: [] }],
  middleware: [session] as const,
  responses: {
    200: json(
      z.object({ integrations: z.array(Integration) }),
      "Every integration the user can connect, with the accounts already connected",
    ),
    401: error("Not signed in"),
  },
});

const getIcon = createRoute({
  operationId: "getIntegrationIcon",
  method: "get",
  path: "/{id}/icon",
  request: { params: IntegrationParams },
  responses: {
    200: {
      content: { "image/png": { schema: z.string().openapi({ format: "binary" }) } },
      description: "The service's logo on a white square",
    },
    404: error("No such integration or no logo"),
  },
});

const connectIntegration = createRoute({
  operationId: "connectIntegration",
  method: "post",
  path: "/{id}/connect",
  security: [{ bearer: [] }],
  middleware: [session] as const,
  request: {
    params: IntegrationParams,
    body: body(
      z.object({
        agentId: z
          .uuid()
          .optional()
          .describe("The agent whose conversation showed the button; it continues once connected"),
      }),
      "Where the connection was started",
    ),
  },
  responses: {
    200: json(
      z.object({ url: z.url() }),
      "Open this in a web authentication session (apps) or the same tab (web). It ends at goat://connected, or at /connected on the web app when a browser started it, with state and code for createConnection",
    ),
    400: error("Invalid request"),
    401: error("Not signed in"),
    404: error("No such integration or agent"),
  },
});

const createConnection = createRoute({
  operationId: "createConnection",
  method: "post",
  path: "/{id}/connections",
  security: [{ bearer: [] }],
  middleware: [session] as const,
  request: {
    params: IntegrationParams,
    body: body(
      z.object({ state: z.string().min(1), code: z.string().min(1) }),
      "The state and code from goat://connected",
    ),
  },
  responses: {
    201: json(z.object({ connection: Connection }), "The connected account"),
    400: error("Invalid request"),
    401: error("Not signed in"),
    404: error("No such attempt: it expired, finished already, or another user started it"),
    502: error("The service did not complete the sign-in"),
  },
});

const receiveAuthorization = createRoute({
  operationId: "receiveAuthorization",
  method: "get",
  path: "/{id}/callback",
  request: {
    params: IntegrationParams,
    query: z.object({
      code: z.string().optional(),
      state: z.string().optional(),
      error: z.string().optional(),
      iss: z.string().optional(),
    }),
  },
  responses: {
    302: {
      description:
        "Back to the client that started it at goat://connected (apps) or <web app>/connected (browsers), with ?integration=<id>&state=…&code=…, or with error=denied|expired|failed",
    },
  },
});

async function isBrowser(deviceId: string | undefined): Promise<boolean> {
  if (!deviceId) return false;
  const [device] = await db
    .select({ platform: devices.platform })
    .from(devices)
    .where(eq(devices.id, deviceId));
  return device?.platform === "web";
}

const app = new OpenAPIHono<SessionEnv>({ defaultHook: invalidRequest });

export default app
  .openapi(listIntegrations, async (c) => {
    const rows = await listConnections(c.var.user.id);
    const list = Object.entries(integrations).map(([id, integration]) => ({
      id,
      name: integration.name,
      iconUrl: new URL(`/integrations/${id}/icon`, env.BETTER_AUTH_URL).href,
      connections: rows
        .filter((row) => row.integrationId === id)
        .map((row) => ({
          id: row.id,
          accountName: row.accountName,
          needsReconnect: row.needsReconnect,
        })),
    }));
    return c.json({ integrations: list }, 200);
  })
  .openapi(getIcon, async (c) => {
    const { id } = c.req.valid("param");
    const icon = Bun.file(new URL(`${id}.png`, ICONS));
    if (!integrations[id] || !(await icon.exists())) {
      return c.json({ error: "icon not found" }, 404);
    }
    return c.body(await icon.arrayBuffer(), 200, {
      "content-type": "image/png",
      "cache-control": "public, max-age=86400",
    });
  })
  .openapi(connectIntegration, async (c) => {
    const { id } = c.req.valid("param");
    const { agentId } = c.req.valid("json");
    if (agentId) {
      const [agent] = await db
        .select({ id: agents.id })
        .from(agents)
        .where(and(eq(agents.id, agentId), eq(agents.userId, c.var.user.id)));
      if (!agent) return c.json({ error: "agent not found" }, 404);
    }
    const url = await startConnecting(c.var.user.id, id, agentId, await isBrowser(c.var.deviceId));
    if (!url) return c.json({ error: "integration not found" }, 404);
    return c.json({ url }, 200);
  })
  .openapi(createConnection, async (c) => {
    const { id } = c.req.valid("param");
    const finished = await finishConnecting(c.var.user.id, id, c.req.valid("json"));
    if (finished.status !== "connected") {
      return finished.status === "unknown"
        ? c.json({ error: "attempt not found" }, 404)
        : c.json({ error: "sign-in failed" }, 502);
    }

    if (finished.agentId) {
      await resumeAfterConnect(finished.agentId, c.var.user.timeZone ?? undefined).catch(
        (cause: unknown) => log.error({ err: cause, agentId: finished.agentId }, "resume failed"),
      );
    }
    const { connection } = finished;
    return c.json(
      {
        connection: {
          id: connection.id,
          accountName: connection.accountName,
          needsReconnect: connection.needsReconnect,
        },
      },
      201,
    );
  })
  .openapi(receiveAuthorization, async (c) => {
    const { id } = c.req.valid("param");
    const back = await returnToClient(id, c.req.valid("query"));
    return c.redirect(back.href, 302);
  });

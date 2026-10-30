import { OpenAPIHono } from "@hono/zod-openapi";
import { HTTPException } from "hono/http-exception";
import { requestId } from "hono/request-id";
import { client } from "./db";
import { env } from "./env";
import { startAgents, stopAgents } from "./lib/agent";
import { auth } from "./lib/auth";
import { logger } from "./lib/logger";
import { document, invalidRequest } from "./lib/openapi";
import { requestLogger } from "./middlewares/request-logger";
import agents from "./routes/agents";
import authRoutes from "./routes/auth";
import me from "./routes/me";

const base = new OpenAPIHono({ defaultHook: invalidRequest });
base.use(requestId(), requestLogger);

export const app = base.route("/auth", authRoutes).route("/me", me).route("/agents", agents);

app.on(["GET", "POST"], "/api/auth/*", (c) => auth.handler(c.req.raw));
app.doc31("/openapi.json", document);

app.openAPIRegistry.registerComponent("securitySchemes", "bearer", {
  type: "http",
  scheme: "bearer",
});

app.notFound((c) => c.json({ error: "not found" }, 404));
app.onError((error, c) => {
  if (error instanceof HTTPException) return c.json({ error: error.message }, error.status);
  logger.error({ err: error, path: c.req.path }, "unhandled error");
  return c.json({ error: "internal error" }, 500);
});

if (import.meta.main) {
  const server = Bun.serve({ port: env.PORT, fetch: app.fetch, idleTimeout: 60 });
  startAgents();
  logger.info({ port: server.port }, "listening");

  const shutdown = async () => {
    await server.stop(true);
    await stopAgents();
    await client.end();
    logger.flush();
    process.exit(0);
  };
  process.once("SIGTERM", () => void shutdown());
  process.once("SIGINT", () => void shutdown());
}

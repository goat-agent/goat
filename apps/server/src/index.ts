import { Hono } from "hono";
import { HTTPException } from "hono/http-exception";
import { client } from "./db";
import { env } from "./env";
import { startAgents, stopAgents } from "./lib/agent";
import { auth } from "./lib/auth";
import agents from "./routes/agents";

export const app = new Hono()
  .on(["GET", "POST"], "/api/auth/*", (c) => auth.handler(c.req.raw))
  .route("/agents", agents)
  .notFound((c) => c.json({ error: "not found" }, 404))
  .onError((error, c) => {
    if (error instanceof HTTPException) return c.json({ error: error.message }, error.status);
    console.error(error);
    return c.json({ error: "internal error" }, 500);
  });

if (import.meta.main) {
  const server = Bun.serve({ port: env.PORT, fetch: app.fetch, idleTimeout: 60 });
  startAgents();

  const shutdown = async () => {
    await server.stop(true);
    await stopAgents();
    await client.end();
    process.exit(0);
  };
  process.once("SIGTERM", () => void shutdown());
  process.once("SIGINT", () => void shutdown());
}

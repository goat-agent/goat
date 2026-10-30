import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import { removeConnection } from "../lib/integrations/connections";
import { error, invalidRequest } from "../lib/openapi";
import { session, type SessionEnv } from "../middlewares/session";

const deleteConnection = createRoute({
  operationId: "deleteConnection",
  method: "delete",
  path: "/{id}",
  security: [{ bearer: [] }],
  request: { params: z.object({ id: z.uuid() }) },
  responses: {
    204: { description: "Disconnected; the service was asked to revoke the token where it can" },
    401: error("Not signed in"),
    404: error("No such connection"),
  },
});

const app = new OpenAPIHono<SessionEnv>({ defaultHook: invalidRequest });
app.use(session);

export default app.openapi(deleteConnection, async (c) => {
  const removed = await removeConnection(c.var.user.id, c.req.valid("param").id);
  if (!removed) return c.json({ error: "connection not found" }, 404);
  return c.body(null, 204);
});

import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import { auth } from "../lib/auth";
import { body, error, invalidRequest, json } from "../lib/openapi";
import { session, type SessionEnv } from "../middlewares/session";

export const User = z
  .object({ id: z.string(), email: z.string(), name: z.string() })
  .openapi("User");

export function publicUser(user: z.infer<typeof User>): z.infer<typeof User> {
  return { id: user.id, email: user.email, name: user.name };
}

const getMe = createRoute({
  operationId: "getMe",
  method: "get",
  path: "/",
  security: [{ bearer: [] }],
  responses: { 200: json(User, "The signed-in user"), 401: error("Not signed in") },
});

const updateMe = createRoute({
  operationId: "updateMe",
  method: "patch",
  path: "/",
  security: [{ bearer: [] }],
  request: {
    body: body(z.object({ name: z.string().trim().min(1).max(100) }), "Profile fields to change"),
  },
  responses: {
    200: json(User, "The updated user"),
    400: error("Invalid request"),
    401: error("Not signed in"),
  },
});

const app = new OpenAPIHono<SessionEnv>({ defaultHook: invalidRequest });
app.use(session);

export default app
  .openapi(getMe, (c) => c.json(publicUser(c.var.user), 200))
  .openapi(updateMe, async (c) => {
    const { name } = c.req.valid("json");
    await auth.api.updateUser({ headers: c.req.raw.headers, body: { name } });
    return c.json(publicUser({ ...c.var.user, name }), 200);
  });

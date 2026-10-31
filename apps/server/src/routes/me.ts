import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import { and, eq, ne } from "drizzle-orm";
import { db } from "../db";
import { devices } from "../db/schema/devices";
import { auth } from "../lib/auth";
import { isTimeZone } from "../lib/dates";
import { moveSchedules } from "../lib/jobs/schedules";
import { body, error, invalidRequest, json } from "../lib/openapi";
import { session, type SessionEnv } from "../middlewares/session";

export const User = z
  .object({ id: z.string(), email: z.string(), name: z.string(), timeZone: z.string().optional() })
  .openapi("User");

export function publicUser(user: typeof auth.$Infer.Session.user): z.infer<typeof User> {
  return { id: user.id, email: user.email, name: user.name, timeZone: user.timeZone ?? undefined };
}

const ProfileChanges = z
  .object({
    name: z.string().trim().min(1).max(100).optional(),
    timeZone: z.string().refine(isTimeZone).optional(),
  })
  .refine((changes) => Object.keys(changes).length > 0);

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
    body: body(ProfileChanges, "Profile fields to change; the time zone is an IANA name"),
  },
  responses: {
    200: json(User, "The updated user"),
    400: error("Invalid request"),
    401: error("Not signed in"),
  },
});

const registerPushToken = createRoute({
  operationId: "registerPushToken",
  method: "put",
  path: "/push-token",
  security: [{ bearer: [] }],
  request: {
    body: body(
      z.object({ token: z.string().regex(/^[0-9a-f]{32,200}$/i) }),
      "This device's APNs token, hex-encoded; send it on every launch",
    ),
  },
  responses: {
    204: { description: "This device gets the user's notifications until it signs out" },
    400: error("Invalid request"),
    401: error("Not signed in"),
    409: error("This session was not signed in as a device"),
  },
});

const app = new OpenAPIHono<SessionEnv>({ defaultHook: invalidRequest });
app.use(session);

export default app
  .openapi(getMe, (c) => c.json(publicUser(c.var.user), 200))
  .openapi(updateMe, async (c) => {
    const changes = c.req.valid("json");
    await auth.api.updateUser({ headers: c.req.raw.headers, body: changes });
    if (changes.timeZone && changes.timeZone !== c.var.user.timeZone) {
      await moveSchedules(c.var.user.id, changes.timeZone);
    }
    return c.json(publicUser({ ...c.var.user, ...changes }), 200);
  })
  .openapi(registerPushToken, async (c) => {
    const { token } = c.req.valid("json");
    const { deviceId } = c.var;
    if (!deviceId) return c.json({ error: "not signed in as a device" }, 409);
    await db.transaction(async (tx) => {
      // The same install signed in to another account must stop getting that account's pushes.
      await tx
        .update(devices)
        .set({ pushToken: null })
        .where(and(eq(devices.pushToken, token), ne(devices.id, deviceId)));
      await tx.update(devices).set({ pushToken: token }).where(eq(devices.id, deviceId));
    });
    return c.body(null, 204);
  });

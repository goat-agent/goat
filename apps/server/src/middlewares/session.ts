import { createMiddleware } from "hono/factory";
import { HTTPException } from "hono/http-exception";
import { auth } from "../lib/auth";

type User = typeof auth.$Infer.Session.user;

export type SessionEnv = { Variables: { user: User; deviceId: string | undefined } };

export const session = createMiddleware<SessionEnv>(async (c, next) => {
  const result = await auth.api.getSession({ headers: c.req.raw.headers });
  if (!result) throw new HTTPException(401, { message: "unauthorized" });
  c.set("user", result.user);
  c.set("deviceId", result.session.deviceId ?? undefined);
  await next();
});

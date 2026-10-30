import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import { isAPIError } from "better-auth/api";
import { auth } from "../lib/auth";
import { body, error, invalidRequest, json } from "../lib/openapi";
import { session, type SessionEnv } from "../middlewares/session";
import { publicUser, User } from "./me";

const Session = z.object({ token: z.string(), user: User }).openapi("Session");

const sendCode = createRoute({
  operationId: "sendCode",
  method: "post",
  path: "/email-code",
  request: { body: body(z.object({ email: z.email() }), "Where to send the sign-in code") },
  responses: { 204: { description: "Code sent" }, 400: error("Invalid request") },
});

const signIn = createRoute({
  operationId: "signIn",
  method: "post",
  path: "/sign-in",
  request: {
    body: body(
      z.object({ email: z.email(), code: z.string().regex(/^\d{6}$/) }),
      "The emailed code",
    ),
  },
  responses: {
    200: json(Session, "Signed in; use the token as a bearer token"),
    400: error("Invalid request"),
    401: error("Wrong or expired code"),
  },
});

const signOut = createRoute({
  operationId: "signOut",
  method: "post",
  path: "/sign-out",
  security: [{ bearer: [] }],
  middleware: [session] as const,
  responses: {
    204: { description: "Signed out; the token no longer works" },
    401: error("Not signed in"),
  },
});

export default new OpenAPIHono<SessionEnv>({ defaultHook: invalidRequest })
  .openapi(sendCode, async (c) => {
    const { email } = c.req.valid("json");
    await auth.api.sendVerificationOTP({
      body: { email, type: "sign-in" },
      headers: c.req.raw.headers,
    });
    return c.body(null, 204);
  })
  .openapi(signIn, async (c) => {
    const { email, code } = c.req.valid("json");
    try {
      const { token, user } = await auth.api.signInEmailOTP({ body: { email, otp: code } });
      return c.json({ token, user: publicUser(user) }, 200);
    } catch (cause) {
      if (isAPIError(cause)) return c.json({ error: "wrong or expired code" }, 401);
      throw cause;
    }
  })
  .openapi(signOut, async (c) => {
    await auth.api.signOut({ headers: c.req.raw.headers });
    return c.body(null, 204);
  });

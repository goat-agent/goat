import { createMiddleware } from "hono/factory";
import type { RequestIdVariables } from "hono/request-id";
import { logger } from "../lib/logger";

const log = logger.child({ module: "http" });

export const requestLogger = createMiddleware<{ Variables: RequestIdVariables }>(
  async (c, next) => {
    const startedAt = performance.now();
    await next();
    log.info(
      {
        requestId: c.var.requestId,
        method: c.req.method,
        path: c.req.path,
        status: c.res.status,
        ms: Math.round(performance.now() - startedAt),
      },
      "request",
    );
  },
);

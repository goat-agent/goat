import { pino } from "pino";
import { env } from "../env";

export const logger = pino({
  level: env.LOG_LEVEL,
  // A safety net: we log ids, counts, and timings, never content or credentials.
  redact: {
    paths: [
      "authorization",
      "*.authorization",
      "*.headers.authorization",
      "token",
      "*.token",
      "otp",
      "*.otp",
      "password",
      "*.password",
      "apiKey",
      "*.apiKey",
    ],
    censor: "[redacted]",
  },
});

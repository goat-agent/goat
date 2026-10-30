import { z } from "zod";

export const env = z
  .object({
    PORT: z.coerce.number().int().positive().default(3000),
    DATABASE_URL: z.url(),
    BETTER_AUTH_URL: z.url(),
    BETTER_AUTH_SECRET: z.string().min(32),
    RESEND_API_KEY: z.string().min(1),
    EMAIL_FROM: z.string().min(1),
    ANTHROPIC_API_KEY: z.string().min(1),
    OPENAI_API_KEY: z.string().min(1),
    FIRECRAWL_API_KEY: z.string().min(1),
    LOG_LEVEL: z
      .enum(["fatal", "error", "warn", "info", "debug", "trace", "silent"])
      .default("info"),
  })
  .parse(process.env);

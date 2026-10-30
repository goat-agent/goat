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
    S3_ENDPOINT: z.url(),
    S3_BUCKET: z.string().min(1),
    S3_REGION: z.string().min(1).default("us-east-1"),
    S3_ACCESS_KEY_ID: z.string().min(1),
    S3_SECRET_ACCESS_KEY: z.string().min(1),
    CREDENTIALS_KEY: z.base64().refine((key) => Buffer.from(key, "base64").length === 32, {
      message: "CREDENTIALS_KEY must be 32 bytes, base64-encoded",
    }),
    GITHUB_CLIENT_ID: z.string().min(1).optional(),
    GITHUB_CLIENT_SECRET: z.string().min(1).optional(),
    LINEAR_CLIENT_ID: z.string().min(1).optional(),
    NOTION_CLIENT_ID: z.string().min(1).optional(),
    SLACK_CLIENT_ID: z.string().min(1).optional(),
    SLACK_CLIENT_SECRET: z.string().min(1).optional(),
    GOOGLE_CLIENT_ID: z.string().min(1).optional(),
    GOOGLE_CLIENT_SECRET: z.string().min(1).optional(),
    TODOIST_CLIENT_ID: z.string().min(1).optional(),
    GRANOLA_CLIENT_ID: z.string().min(1).optional(),
    READWISE_CLIENT_ID: z.string().min(1).optional(),
    DROPBOX_CLIENT_ID: z.string().min(1).optional(),
    DROPBOX_CLIENT_SECRET: z.string().min(1).optional(),
    CANVA_CLIENT_ID: z.string().min(1).optional(),
    MICROSOFT_CLIENT_ID: z.string().min(1).optional(),
    MICROSOFT_CLIENT_SECRET: z.string().min(1).optional(),
    LOG_LEVEL: z
      .enum(["fatal", "error", "warn", "info", "debug", "trace", "silent"])
      .default("info"),
  })
  .parse(process.env);

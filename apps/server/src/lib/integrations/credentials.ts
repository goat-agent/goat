import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";
import { z } from "zod";
import { env } from "../../env";

export const Credentials = z.object({
  type: z.literal("oauth"),
  accessToken: z.string(),
  refreshToken: z.string().optional(),
  expiresAt: z.iso.datetime().optional(),
  scope: z.string().optional(),
});

export type Credentials = z.infer<typeof Credentials>;

const ALGORITHM = "aes-256-gcm";
const IV_BYTES = 12;
const TAG_BYTES = 16;
const key = Buffer.from(env.CREDENTIALS_KEY, "base64");

// The version prefix lets a later key or algorithm take over while older rows still open.
export function seal(credentials: Credentials): string {
  const iv = randomBytes(IV_BYTES);
  const cipher = createCipheriv(ALGORITHM, key, iv);
  const body = Buffer.concat([
    cipher.update(JSON.stringify(credentials), "utf8"),
    cipher.final(),
    cipher.getAuthTag(),
  ]);
  return `v1.${iv.toString("base64url")}.${body.toString("base64url")}`;
}

export function open(sealed: string): Credentials {
  const [version, iv, body] = sealed.split(".");
  if (version !== "v1" || !iv || !body) throw new Error("unknown credentials format");

  const bytes = Buffer.from(body, "base64url");
  const decipher = createDecipheriv(ALGORITHM, key, Buffer.from(iv, "base64url"));
  decipher.setAuthTag(bytes.subarray(-TAG_BYTES));
  const json = Buffer.concat([
    decipher.update(bytes.subarray(0, -TAG_BYTES)),
    decipher.final(),
  ]).toString("utf8");
  return Credentials.parse(JSON.parse(json));
}

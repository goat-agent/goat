import { and, eq, exists, gt, isNotNull, sql } from "drizzle-orm";
import { db } from "../../db";
import { sessions } from "../../db/schema/auth";
import { devices } from "../../db/schema/devices";
import { logger } from "../logger";
import * as apns from "./apns";

export type Push = {
  title: string;
  /** Shown on the lock screen, so it passes through Apple: the first bubble of a reply. */
  body: string;
  agentId: string;
  /** The same push sent twice shows once. */
  collapseId: string;
};

const log = logger.child({ module: "push" });

/** Sends to every device the user is signed in on. Does nothing until push is configured. */
export async function send(userId: string, push: Push): Promise<void> {
  if (!apns.isConfigured) return;
  const signedIn = db
    .select({ id: sessions.id })
    .from(sessions)
    .where(
      and(eq(sessions.deviceId, sql`${devices.id}::text`), gt(sessions.expiresAt, new Date())),
    );
  const rows = await db
    .select({ token: devices.pushToken })
    .from(devices)
    .where(and(eq(devices.userId, userId), isNotNull(devices.pushToken), exists(signedIn)));
  const tokens = rows.flatMap(({ token }) => (token ? [token] : []));
  await Promise.all(
    tokens.map(async (token) => {
      try {
        const result = await apns.send(token, push);
        if (result === "unregistered") {
          await db.update(devices).set({ pushToken: null }).where(eq(devices.pushToken, token));
        }
        if (result !== "sent") log.warn({ userId, result }, "push not delivered");
      } catch (error) {
        log.warn({ err: error, userId }, "push failed");
      }
    }),
  );
}

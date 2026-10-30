import { and, eq, isNull } from "drizzle-orm";
import { db } from "../../db";
import { sessions } from "../../db/schema/auth";
import { deviceCalls, devices } from "../../db/schema/devices";
import { INTERRUPTED, RpcResponse } from "./protocol";
import { announceCall, announceRemoval, watchCall } from "./signals";

export type Device = typeof devices.$inferSelect;

// A connected device claims a call within milliseconds; silence this long means no device is there.
const PICKUP_MS = 1_000;

export class DeviceOffline extends Error {}
export class DeviceStoppedResponding extends Error {}

/** An error the device reported while carrying out a call. */
export class DeviceFailed extends Error {}

export async function registerDevice(
  userId: string,
  input: { installationId: string; name: string; platform: string },
): Promise<string> {
  const [device] = await db
    .insert(devices)
    .values({ userId, ...input })
    .onConflictDoUpdate({
      target: [devices.userId, devices.installationId],
      set: { name: input.name, platform: input.platform },
    })
    .returning({ id: devices.id });
  if (!device) throw new Error("device upsert returned no row");
  return device.id;
}

export async function attachSession(token: string, deviceId: string): Promise<void> {
  await db.update(sessions).set({ deviceId }).where(eq(sessions.token, token));
}

export function listDevices(userId: string): Promise<Device[]> {
  return db.select().from(devices).where(eq(devices.userId, userId)).orderBy(devices.createdAt);
}

/** Signs the device out and forgets it. False when the user has no such device. */
export async function removeDevice(userId: string, deviceId: string): Promise<boolean> {
  return db.transaction(async (tx) => {
    const removed = await tx
      .delete(devices)
      .where(and(eq(devices.id, deviceId), eq(devices.userId, userId)))
      .returning({ id: devices.id });
    if (removed.length === 0) return false;
    await tx.delete(sessions).where(eq(sessions.deviceId, deviceId));
    await announceRemoval(tx, deviceId);
    return true;
  });
}

/**
 * Runs `method` on the device and returns its result. Throws `DeviceOffline` when no connection
 * claims the call in time, and then the call can never run; `DeviceStoppedResponding` when a
 * connection claimed it but no answer came, so it may or may not have run.
 */
export async function call(
  deviceId: string,
  method: string,
  params: unknown,
  options: { timeoutMs: number },
): Promise<unknown> {
  const [row] = await db
    .insert(deviceCalls)
    .values({ deviceId, method, params })
    .returning({ id: deviceCalls.id });
  if (!row) throw new Error("device call insert returned no row");

  const watch = await watchCall(row.id);
  try {
    await announceCall(deviceId, row.id);
    const pickupDeadline = Date.now() + PICKUP_MS;
    const answerDeadline = pickupDeadline + options.timeoutMs;
    for (;;) {
      const [state] = await db
        .select({ pickedAt: deviceCalls.pickedAt, response: deviceCalls.response })
        .from(deviceCalls)
        .where(eq(deviceCalls.id, row.id));
      if (state?.response) return resultOf(state.response);
      if (!state?.pickedAt && Date.now() >= pickupDeadline && (await withdraw(row.id))) {
        throw new DeviceOffline();
      }
      if (Date.now() >= answerDeadline) throw new DeviceStoppedResponding();
      await watch.next((state?.pickedAt ? answerDeadline : pickupDeadline) - Date.now());
    }
  } finally {
    watch.stop();
    await db.delete(deviceCalls).where(eq(deviceCalls.id, row.id));
  }
}

function resultOf(stored: unknown): unknown {
  const { error, result } = RpcResponse.parse(stored);
  if (error?.code === INTERRUPTED) throw new DeviceStoppedResponding();
  if (error) throw new DeviceFailed(error.message);
  return result;
}

/** True when the call was still unclaimed, so no device can run it now. */
async function withdraw(callId: string): Promise<boolean> {
  const withdrawn = await db
    .delete(deviceCalls)
    .where(and(eq(deviceCalls.id, callId), isNull(deviceCalls.pickedAt)))
    .returning({ id: deviceCalls.id });
  return withdrawn.length > 0;
}

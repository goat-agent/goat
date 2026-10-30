import { sql } from "drizzle-orm";
import { client, db, type Transaction } from "../../db";

// Notifications only say that something changed; readers look at device_calls for the state, so a
// notification missed during a reconnect costs a wait, not a wrong answer.
const CALLS = "device_calls";
const PROGRESS = "device_call_progress";
const REMOVED = "device_removed";

type Wake = () => void;
type DeviceListener = { onCall: (callId: string) => void; onRemoved: () => void };

const deviceListeners = new Map<string, Set<DeviceListener>>();
const progress = new Map<string, Set<Wake>>();
let listening: Promise<unknown> | undefined;

function listen(): Promise<unknown> {
  listening ??= Promise.all([
    client.listen(CALLS, (payload) => {
      const [deviceId, callId] = payload.split(" ");
      if (!deviceId || !callId) return;
      for (const listener of deviceListeners.get(deviceId) ?? []) listener.onCall(callId);
    }),
    client.listen(REMOVED, (deviceId) => {
      for (const listener of deviceListeners.get(deviceId) ?? []) listener.onRemoved();
    }),
    client.listen(
      PROGRESS,
      (callId) => {
        for (const wake of progress.get(callId) ?? []) wake();
      },
      // Runs again after a reconnect, when progress may have been missed.
      () => {
        for (const set of progress.values()) for (const wake of set) wake();
      },
    ),
  ]).catch((error: unknown) => {
    listening = undefined;
    throw error;
  });
  return listening;
}

export async function announceCall(deviceId: string, callId: string): Promise<void> {
  await db.execute(sql`select pg_notify(${CALLS}, ${`${deviceId} ${callId}`})`);
}

export async function announceRemoval(tx: Transaction, deviceId: string): Promise<void> {
  await tx.execute(sql`select pg_notify(${REMOVED}, ${deviceId})`);
}

export async function announceProgress(tx: Transaction, callId: string): Promise<void> {
  await tx.execute(sql`select pg_notify(${PROGRESS}, ${callId})`);
}

/** Hears calls made to the device and its removal until the returned stop runs. */
export async function watchDevice(deviceId: string, listener: DeviceListener): Promise<() => void> {
  await listen();
  const set = deviceListeners.get(deviceId) ?? new Set();
  set.add(listener);
  deviceListeners.set(deviceId, set);
  return () => {
    set.delete(listener);
    if (set.size === 0) deviceListeners.delete(deviceId);
  };
}

/** `next` resolves when the call may have progressed, or after `ms`. */
export async function watchCall(
  callId: string,
): Promise<{ next: (ms: number) => Promise<void>; stop: () => void }> {
  await listen();
  const set = progress.get(callId) ?? new Set();
  progress.set(callId, set);
  let pending = false;
  let resolve: Wake | undefined;
  const wake: Wake = () => {
    pending = true;
    resolve?.();
  };
  set.add(wake);

  const next = (ms: number) =>
    new Promise<void>((done) => {
      if (pending) {
        pending = false;
        done();
        return;
      }
      const timer = setTimeout(finish, Math.max(ms, 0));
      function finish() {
        clearTimeout(timer);
        pending = false;
        resolve = undefined;
        done();
      }
      resolve = finish;
    });

  const stop = () => {
    set.delete(wake);
    if (set.size === 0) progress.delete(callId);
  };
  return { next, stop };
}

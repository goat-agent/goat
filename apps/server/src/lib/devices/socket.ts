import type { ServerWebSocket, WebSocketHandler } from "bun";
import { and, eq, isNull, sql } from "drizzle-orm";
import { db } from "../../db";
import { deviceCalls } from "../../db/schema/devices";
import { logger } from "../logger";
import { INTERRUPTED, RpcResponse } from "./protocol";
import { announceProgress, watchDevice } from "./signals";

export type SocketData = { deviceId: string; pending: Set<string>; stop?: () => void };

const log = logger.child({ module: "devices" });

/** The server side of a device's WebSocket: it delivers calls made to the device and stores the answers. */
export const deviceSockets: WebSocketHandler<SocketData> = {
  // Bun pings idle sockets; a device that stops answering pings is dropped after this many seconds.
  idleTimeout: 30,
  sendPings: true,
  async open(ws) {
    const stop = await watchDevice(ws.data.deviceId, {
      onCall: (callId) => void deliver(ws, callId),
      onRemoved: () => ws.close(4001, "device removed"),
    });
    if (ws.readyState === WebSocket.OPEN) ws.data.stop = stop;
    else stop();
    log.info({ deviceId: ws.data.deviceId }, "device connected");
  },
  message(ws, message) {
    void answer(ws, message);
  },
  close(ws) {
    ws.data.stop?.();
    void interrupt(ws.data);
    log.info({ deviceId: ws.data.deviceId }, "device disconnected");
  },
};

async function deliver(ws: ServerWebSocket<SocketData>, callId: string): Promise<void> {
  try {
    const call = await db.transaction(async (tx) => {
      const [claimed] = await tx
        .update(deviceCalls)
        .set({ pickedAt: sql`clock_timestamp()` })
        .where(
          and(
            eq(deviceCalls.id, callId),
            eq(deviceCalls.deviceId, ws.data.deviceId),
            isNull(deviceCalls.pickedAt),
          ),
        )
        .returning({ method: deviceCalls.method, params: deviceCalls.params });
      if (claimed) await announceProgress(tx, callId);
      return claimed;
    });
    if (!call) return;
    ws.data.pending.add(callId);
    ws.send(JSON.stringify({ jsonrpc: "2.0", id: callId, ...call }));
  } catch (error) {
    log.error({ err: error, deviceId: ws.data.deviceId, callId }, "delivering a call failed");
  }
}

async function answer(ws: ServerWebSocket<SocketData>, message: string | Buffer): Promise<void> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(message.toString());
  } catch {
    // Not JSON-RPC; there is no id to answer.
    return;
  }
  const response = RpcResponse.safeParse(parsed);
  if (!response.success || !ws.data.pending.delete(response.data.id)) return;
  try {
    await store(response.data);
  } catch (error) {
    log.error({ err: error, deviceId: ws.data.deviceId }, "storing an answer failed");
  }
}

async function interrupt(data: SocketData): Promise<void> {
  const ids = [...data.pending];
  data.pending.clear();
  for (const id of ids) {
    try {
      await store({
        jsonrpc: "2.0",
        id,
        error: { code: INTERRUPTED, message: "The device disconnected before answering." },
      });
    } catch (error) {
      log.error({ err: error, deviceId: data.deviceId }, "storing an interruption failed");
    }
  }
}

async function store(response: RpcResponse): Promise<void> {
  await db.transaction(async (tx) => {
    const updated = await tx
      .update(deviceCalls)
      .set({ response })
      .where(and(eq(deviceCalls.id, response.id), isNull(deviceCalls.response)))
      .returning({ id: deviceCalls.id });
    if (updated.length > 0) await announceProgress(tx, response.id);
  });
}

import { createRoute, OpenAPIHono, z } from "@hono/zod-openapi";
import type { Server } from "bun";
import { listDevices, removeDevice } from "../lib/devices";
import type { SocketData } from "../lib/devices/socket";
import { error, invalidRequest, json, openEnum } from "../lib/openapi";
import { session, type SessionEnv } from "../middlewares/session";

const Device = z
  .object({
    id: z.uuid(),
    name: z.string(),
    platform: openEnum(["ios", "macos", "web"]),
    isCurrent: z.boolean().describe("True for the device that made this request"),
    createdAt: z.iso.datetime(),
  })
  .openapi("Device");

const listDevicesRoute = createRoute({
  operationId: "listDevices",
  method: "get",
  path: "/",
  security: [{ bearer: [] }],
  responses: {
    200: json(z.object({ devices: z.array(Device) }), "Devices signed in to the account"),
    401: error("Not signed in"),
  },
});

const removeDeviceRoute = createRoute({
  operationId: "removeDevice",
  method: "delete",
  path: "/{id}",
  security: [{ bearer: [] }],
  request: { params: z.object({ id: z.uuid() }) },
  responses: {
    204: { description: "Removed; the device is signed out" },
    401: error("Not signed in"),
    404: error("No such device"),
  },
});

const connectDevice = createRoute({
  operationId: "connectDevice",
  method: "get",
  path: "/connect",
  security: [{ bearer: [] }],
  description:
    "Upgrades to a WebSocket that carries JSON-RPC 2.0. The server sends requests (`RpcRequest`), and the device answers each with an `RpcResponse` carrying the same id. Sign in with a `device` first.",
  responses: {
    101: { description: "Switching to the WebSocket" },
    401: error("Not signed in"),
    409: error("The session was not signed in from a device"),
    426: error("Not a WebSocket request"),
  },
});

const app = new OpenAPIHono<SessionEnv & { Bindings: Server<SocketData> }>({
  defaultHook: invalidRequest,
});
app.use(session);

export default app
  .openapi(listDevicesRoute, async (c) => {
    const rows = await listDevices(c.var.user.id);
    const devices = rows.map((device) => ({
      id: device.id,
      name: device.name,
      platform: device.platform,
      isCurrent: device.id === c.var.deviceId,
      createdAt: device.createdAt.toISOString(),
    }));
    return c.json({ devices }, 200);
  })
  .openapi(removeDeviceRoute, async (c) => {
    const { id } = c.req.valid("param");
    if (!(await removeDevice(c.var.user.id, id))) return c.json({ error: "device not found" }, 404);
    return c.body(null, 204);
  })
  .openapi(connectDevice, (c) => {
    const deviceId = c.var.deviceId;
    if (!deviceId) return c.json({ error: "sign in with a device first" }, 409);
    if (!c.env.upgrade(c.req.raw, { data: { deviceId, pending: new Set() } })) {
      return c.json({ error: "expected a WebSocket request" }, 426);
    }
    return new Response(null);
  });

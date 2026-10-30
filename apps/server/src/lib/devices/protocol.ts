import { z } from "@hono/zod-openapi";

// JSON-RPC 2.0 over a device's WebSocket. The server sends requests; the device answers each one
// with the same id.

/** The device's connection closed while it was carrying out the call. */
export const INTERRUPTED = -32001;

export const RpcRequest = z
  .object({ jsonrpc: z.literal("2.0"), id: z.string(), method: z.string(), params: z.unknown() })
  .openapi("RpcRequest");

const RpcError = z.object({ code: z.int(), message: z.string() }).openapi("RpcError");

export const RpcResponse = z
  .object({
    jsonrpc: z.literal("2.0"),
    id: z.string(),
    result: z.unknown().optional(),
    error: RpcError.optional(),
  })
  .openapi("RpcResponse");

export type RpcResponse = z.infer<typeof RpcResponse>;

/** Registered in the OpenAPI document, keyed by name, so the apps get these types generated. */
export const schemas = { RpcRequest, RpcResponse };

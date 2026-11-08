import createClient from "openapi-fetch";
import type { z } from "zod";
import { env } from "@/shared/config/env";
import type { components, paths } from "./schema.gen";

export const apiBase = env.VITE_API_URL;

export const api = createClient<paths>({ baseUrl: apiBase, credentials: "include" });

export type Agent = components["schemas"]["Agent"];
export type Message = components["schemas"]["Message"];
export type SentFile = components["schemas"]["File"];
export type ConnectRequest = components["schemas"]["ConnectRequest"];
export type Integration = components["schemas"]["Integration"];
export type Connection = components["schemas"]["Connection"];
export type Device = components["schemas"]["Device"];
export type User = components["schemas"]["User"];

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number) {
    super(`request failed with ${status}`);
    this.status = status;
  }
}

export async function unwrap<T>(
  request: Promise<{ data?: T; response: Response }>,
): Promise<NonNullable<T>> {
  const { data: body, response } = await request;
  if (!response.ok || body === undefined || body === null) throw new ApiError(response.status);
  return body;
}

export async function expectOk(request: Promise<{ response: Response }>): Promise<void> {
  const { response } = await request;
  if (!response.ok) throw new ApiError(response.status);
}

// openapi-fetch types binary fields as strings, so uploads use fetch and check the schema here.
export async function upload<T>(
  method: "POST" | "PUT",
  path: string,
  form: FormData,
  schema: z.ZodType<T>,
): Promise<T> {
  const response = await fetch(`${apiBase}${path}`, {
    method,
    body: form,
    credentials: "include",
  });
  if (!response.ok) throw new ApiError(response.status);
  return schema.parse(await response.json());
}

export function avatarUrl(agent: Pick<Agent, "id" | "avatarId">): string {
  return `${apiBase}/agents/${agent.id}/avatars/${agent.avatarId}`;
}

export function fileUrl(agentId: string, fileId: string): string {
  return `${apiBase}/agents/${agentId}/files/${fileId}`;
}

import { delay, HttpResponse, sse } from "msw";
import { setupWorker } from "msw/browser";
import { createOpenApiHttp } from "openapi-msw";
import { apiBase, type Message } from "../client";
import type { paths } from "../schema.gen";
import {
  agents,
  conversations,
  devices,
  faceSvg,
  iconSvg,
  integrations,
  me,
  screenshotSvg,
} from "./data";

// `dev:mock` runs the app with no server: every request is answered here from the data in
// ./data.ts, and replies are made up, so every screen state can be seen and worked on.

const http = createOpenApiHttp<paths>({ baseUrl: apiBase });
const listeners = new Set<() => void>();
let nextId = 100;

function changed(): void {
  for (const listener of listeners) listener();
}

function svg(body: string): HttpResponse<string> {
  return new HttpResponse(body, { headers: { "content-type": "image/svg+xml" } });
}

function agentList() {
  return agents
    .map((agent) => ({ ...agent, lastMessage: conversations.get(agent.id)?.at(-1) }))
    .toSorted((a, b) =>
      (b.lastMessage?.createdAt ?? b.createdAt).localeCompare(
        a.lastMessage?.createdAt ?? a.createdAt,
      ),
    );
}

function add(agentId: string, message: Omit<Message, "id" | "createdAt">): void {
  const list = conversations.get(agentId) ?? [];
  list.push({ ...message, id: nextId++, createdAt: new Date().toISOString() });
  conversations.set(agentId, list);
  changed();
}

function setStatus(agentId: string, status: string): void {
  const agent = agents.find(({ id }) => id === agentId);
  if (agent) agent.status = status;
  changed();
}

async function reply(agentId: string, text: string): Promise<void> {
  await delay(400);
  add(agentId, { role: "user", text });
  setStatus(agentId, "running");
  await delay(1800);
  add(agentId, {
    role: "assistant",
    text: `알겠어요. “${text}”는 가짜 서버라 여기까지만 할 수 있어요.`,
  });
  setStatus(agentId, "idle");
}

const handlers = [
  http.get("/me", ({ response }) => response(200).json(me)),
  http.patch("/me", async ({ request, response }) => {
    Object.assign(me, await request.json());
    return response(200).json(me);
  }),
  http.post("/auth/sign-out", ({ response }) => response(204).empty()),
  http.get("/agents", ({ response }) => response(200).json({ agents: agentList() })),
  http.post("/agents", async ({ request, response }) => {
    const body = await request.json();
    const agent = {
      id: crypto.randomUUID(),
      name: body.name ?? "Goat",
      instructions: body.instructions ?? "",
      status: "idle",
      createdAt: new Date().toISOString(),
    };
    agents.unshift(agent);
    changed();
    return response(201).json({ agent });
  }),
  http.patch("/agents/{id}", async ({ params, request, response }) => {
    const agent = agents.find(({ id }) => id === params.id);
    if (!agent) return response(404).json({ error: "agent not found" });
    Object.assign(agent, await request.json());
    changed();
    return response(200).json({ agent });
  }),
  http.delete("/agents/{id}", ({ params, response }) => {
    const index = agents.findIndex(({ id }) => id === params.id);
    if (index >= 0) agents.splice(index, 1);
    changed();
    return response(204).empty();
  }),
  http.untyped.get(`${apiBase}/agents/:id/avatars/:avatarId`, ({ params }) =>
    svg(faceSvg(String(params.avatarId))),
  ),
  http.untyped.get(`${apiBase}/agents/:id/files/:fileId`, () => svg(screenshotSvg)),
  http.untyped.get(`${apiBase}/integrations/:id/icon`, ({ params }) =>
    svg(iconSvg(String(params.id).charAt(0).toUpperCase())),
  ),
  http.get("/agents/{id}/messages", ({ params, query, response }) => {
    const before = Number(query.get("before") ?? Infinity);
    const limit = Number(query.get("limit") ?? 50);
    const list = (conversations.get(params.id) ?? []).filter(({ id }) => id < before);
    return response(200).json({ messages: list.slice(-limit) });
  }),
  http.post("/agents/{id}/messages", async ({ params, request, response }) => {
    const { text = "" } = await request.json();
    void reply(params.id, text);
    return response(202).empty();
  }),
  http.post("/agents/{id}/files", async ({ request, response }) => {
    const form = await request.formData();
    const file = form.get("file");
    const name = file instanceof File ? file.name : "file";
    const mediaType =
      file instanceof File ? file.type || "application/octet-stream" : "application/octet-stream";
    await delay(600);
    return response(201).json({ file: { id: crypto.randomUUID(), name, mediaType } });
  }),
  http.post("/agents/{id}/transcriptions", async ({ response }) => {
    await delay(800);
    return response(200).json({ text: "가짜 서버가 받아쓴 문장이에요" });
  }),
  http.get("/integrations", ({ response }) => response(200).json({ integrations })),
  http.post("/integrations/{id}/connect", ({ response }) =>
    response(200).json({ url: `${location.origin}/connected?integration=notion&error=denied` }),
  ),
  http.delete("/connections/{id}", ({ params, response }) => {
    for (const integration of integrations) {
      integration.connections = integration.connections.filter(({ id }) => id !== params.id);
    }
    return response(204).empty();
  }),
  http.get("/devices", ({ response }) => response(200).json({ devices })),
  http.delete("/devices/{id}", ({ params, response }) => {
    const index = devices.findIndex(({ id }) => id === params.id);
    if (index >= 0) devices.splice(index, 1);
    return response(204).empty();
  }),
  sse<{ agents: string }>(`${apiBase}/agents/events`, ({ client }) => {
    const send = () =>
      client.send({
        event: "agents",
        data: JSON.stringify({ type: "agents", agents: agentList() }),
      });
    send();
    listeners.add(send);
  }),
  sse<{ message: string; status: string }>(
    `${apiBase}/agents/:id/events`,
    ({ client, params, request }) => {
      const agentId = String(params.id);
      let cursor = Number(new URL(request.url).searchParams.get("after") ?? 0);
      let status: string | undefined;
      const send = () => {
        for (const message of conversations.get(agentId) ?? []) {
          if (message.id <= cursor) continue;
          cursor = message.id;
          client.send({
            event: "message",
            id: String(message.id),
            data: JSON.stringify({ type: "message", message }),
          });
        }
        const current = agents.find(({ id }) => id === agentId)?.status;
        if (current && current !== status) {
          status = current;
          client.send({
            event: "status",
            data: JSON.stringify({ type: "status", status: current }),
          });
        }
      };
      send();
      listeners.add(send);
    },
  ),
];

export async function startMock(): Promise<void> {
  await setupWorker(...handlers).start({ quiet: true });
}

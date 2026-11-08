import { afterAll, beforeEach, describe, expect, test } from "bun:test";
import { eq } from "drizzle-orm";
import { db } from "../src/db";
import { messages } from "../src/db/schema/agents";
import * as devices from "../src/lib/devices";
import { RpcRequest } from "../src/lib/devices/protocol";
import { deviceSockets } from "../src/lib/devices/socket";
import { app } from "../src/index";
import { llm } from "./fakes";
import {
  bearer,
  client,
  codeFor,
  createAgent,
  listMessages,
  resetDatabase,
  send,
  setName,
  status,
  waitForIdle,
} from "./helpers";

const server = Bun.serve({ port: 0, fetch: app.fetch, websocket: deviceSockets });

afterAll(async () => {
  await server.stop(true);
});

type Handler = (method: string, params: unknown) => unknown;

/** A computer on the other end of the device WebSocket. */
async function connect(token: string, handle: Handler): Promise<WebSocket> {
  const socket = new WebSocket(`ws://localhost:${server.port}/devices/connect`, {
    headers: { authorization: `Bearer ${token}` },
  });
  socket.addEventListener("message", (event) => {
    const request = RpcRequest.parse(JSON.parse(String(event.data)));
    const result = handle(request.method, request.params);
    socket.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
  });
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve);
    socket.addEventListener("error", reject);
  });
  // The server starts listening for calls just after the socket opens.
  await Bun.sleep(50);
  return socket;
}

async function signInDevice(email: string, name: string, platform = "macos") {
  await client.auth["email-code"].$post({ json: { email } });
  const response = await client.auth["sign-in"].$post({
    json: {
      email,
      code: codeFor(email),
      device: { installationId: crypto.randomUUID(), name, platform },
    },
  });
  if (!response.ok) throw new Error(`sign-in failed with ${response.status}`);
  const { token } = await response.json();
  await setName(token, "민호");
  const listed = await client.devices.$get(undefined, bearer(token));
  if (!listed.ok) throw new Error(`list devices failed with ${listed.status}`);
  const current = (await listed.json()).devices.find((device) => device.isCurrent);
  if (!current) throw new Error("the signed-in device is not listed");
  return { token, deviceId: current.id };
}

const image = { data: "aW1hZ2U=", width: 1280, height: 800 };

const keynote = {
  ref: "win-7",
  app: "Keynote",
  title: "3분기 발표",
  isFront: false,
  isMinimized: false,
};

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
});

describe("devices", () => {
  test("signing in with a device lists it, and removing it signs it out", async () => {
    const { token, deviceId } = await signInDevice("minho@example.com", "Mac mini");

    const removed = await client.devices[":id"].$delete({ param: { id: deviceId } }, bearer(token));
    expect(status(removed)).toBe(204);
    const after = await client.devices.$get(undefined, bearer(token));
    expect(status(after)).toBe(401);
  });

  test("signing in again from the same install keeps the device", async () => {
    const installationId = crypto.randomUUID();
    const email = "minho@example.com";
    const ids: string[] = [];
    for (const name of ["Mac mini", "Studio"]) {
      await client.auth["email-code"].$post({ json: { email } });
      const response = await client.auth["sign-in"].$post({
        json: { email, code: codeFor(email), device: { installationId, name, platform: "macos" } },
      });
      if (!response.ok) throw new Error("sign-in failed");
      const { token } = await response.json();
      const listed = await client.devices.$get(undefined, bearer(token));
      if (!listed.ok) throw new Error("list failed");
      const body = await listed.json();
      ids.push(...body.devices.map((device) => `${device.id} ${device.name}`));
    }
    expect(ids[0]?.split(" ")[0]).toBe(ids[1]?.split(" ")[0]);
    expect(ids[1]).toEndWith("Studio");
  });

  test("a call runs on the connected device and returns its answer", async () => {
    const { token, deviceId } = await signInDevice("minho@example.com", "Mac mini");
    const socket = await connect(token, (method) => ({ echoed: method }));

    const result = await devices.call(deviceId, "computer.status", {}, { timeoutMs: 2_000 });
    expect(result).toEqual({ echoed: "computer.status" });
    socket.close();
  });

  test("a call to a device that is not connected fails quickly and never runs later", async () => {
    const { token, deviceId } = await signInDevice("minho@example.com", "Mac mini");

    const started = Date.now();
    const error = await devices
      .call(deviceId, "computer.click", {}, { timeoutMs: 5_000 })
      .catch((cause: unknown) => cause);
    expect(error).toBeInstanceOf(devices.DeviceOffline);
    expect(Date.now() - started).toBeLessThan(2_000);

    const seen: string[] = [];
    const socket = await connect(token, (method) => seen.push(method));
    await Bun.sleep(200);
    expect(seen).toEqual([]);
    socket.close();
  });

  test("a device that disconnects mid-call leaves the outcome unknown", async () => {
    const { token, deviceId } = await signInDevice("minho@example.com", "Mac mini");
    const socket = new WebSocket(`ws://localhost:${server.port}/devices/connect`, {
      headers: { authorization: `Bearer ${token}` },
    });
    socket.addEventListener("message", () => socket.close());
    await new Promise((resolve) => socket.addEventListener("open", resolve));
    await Bun.sleep(50);

    const error = await devices
      .call(deviceId, "computer.click", {}, { timeoutMs: 5_000 })
      .catch((cause: unknown) => cause);
    expect(error).toBeInstanceOf(devices.DeviceStoppedResponding);
  });

  test("a session without a device cannot open the device socket", async () => {
    const email = "minho@example.com";
    await client.auth["email-code"].$post({ json: { email } });
    const response = await client.auth["sign-in"].$post({ json: { email, code: codeFor(email) } });
    if (!response.ok) throw new Error("sign-in failed");
    const { token } = await response.json();

    const answer = await fetch(`http://localhost:${server.port}/devices/connect`, bearer(token));
    expect(answer.status).toBe(409);
  });
});

describe("computer tools", () => {
  test("the agent lists windows, looks at one, clicks a point, and the screens shrink afterwards", async () => {
    const { token, deviceId } = await signInDevice("minho@example.com", "Mac mini");
    const agentId = await createAgent(token);
    const received: { method: string; params: unknown }[] = [];
    const socket = await connect(token, (method, params) => {
      received.push({ method, params });
      if (method === "computer.status") return { isAvailable: true };
      if (method === "computer.windows") return { windows: [keynote] };
      if (method === "computer.view") return { window: keynote, image };
      return { delivery: "background", view: { window: keynote, image } };
    });

    llm.reply = (prompt) => {
      const results = prompt.filter((message) => message.role === "tool");
      if (results.length === 0) return Promise.resolve({ toolName: "computer_view", input: {} });
      const seen = JSON.stringify(results.at(-1));
      const window = /Keynote — \\"3분기 발표\\" \[(w\d+)\]/.exec(seen)?.[1];
      if (results.length === 1) {
        return Promise.resolve({ toolName: "computer_view", input: { window } });
      }
      if (results.length === 2) {
        return Promise.resolve({
          toolName: "computer_click",
          input: { window, x: 640, y: 120, intent: "내보내기 누르기" },
        });
      }
      return Promise.resolve("PDF로 내보냈어요.");
    };

    await send(token, agentId, "키노트 PDF로 내보내줘");
    await waitForIdle(agentId);

    expect(received.map(({ method }) => method)).toEqual([
      "computer.status",
      "computer.windows",
      "computer.view",
      "computer.click",
    ]);
    expect(received[2]?.params).toEqual({ window: "win-7" });
    expect(received[3]?.params).toEqual({ window: "win-7", at: { x: 640, y: 120 } });

    const shown = await listMessages(token, agentId);
    expect(shown.flatMap((message) => message.steps ?? [])).toEqual(["내보내기 누르기"]);
    expect(shown.at(-1)?.text).toBe("PDF로 내보냈어요.");

    const stored = await db.select().from(messages).where(eq(messages.agentId, agentId));
    const results = stored.flatMap(({ message }) =>
      message.role === "tool" ? message.content.filter((part) => part.type === "tool-result") : [],
    );
    expect(results.map((part) => part.output.type)).toEqual(["text", "text", "text"]);
    expect(JSON.stringify(results)).toContain("(screen removed after the task)");
    expect(JSON.stringify(results)).not.toContain(image.data);
    expect(deviceId).toBeTruthy();
    socket.close();
  });

  test("an action the app takes only in front tells the agent how to ask for it", async () => {
    const { token } = await signInDevice("minho@example.com", "Mac mini");
    const agentId = await createAgent(token);
    const socket = await connect(token, (method) => {
      if (method === "computer.status") return { isAvailable: true };
      if (method === "computer.windows") return { windows: [keynote] };
      return { notDone: "needs_foreground" };
    });
    llm.reply = (prompt) => {
      const results = prompt.filter((message) => message.role === "tool");
      if (results.length === 0) return Promise.resolve({ toolName: "computer_view", input: {} });
      const window = /\[(w\d+)\]/.exec(JSON.stringify(results.at(-1)))?.[1];
      if (results.length === 1) {
        return Promise.resolve({
          toolName: "computer_scroll",
          input: { window, x: 10, y: 10, direction: "down", amount: 3, intent: "내리기" },
        });
      }
      return Promise.resolve("알겠어요.");
    };

    await send(token, agentId, "내려줘");
    await waitForIdle(agentId);
    const result = JSON.stringify(
      llm.prompts.at(-1)?.findLast((message) => message.role === "tool"),
    );
    expect(result).toContain("Keynote does not take this while it is in the background");
    expect(result).toContain("foreground: true");
    socket.close();
  });

  test("an id the run never saw asks the agent to look again", async () => {
    const { token } = await signInDevice("minho@example.com", "Mac mini");
    const agentId = await createAgent(token);
    llm.reply = (prompt) =>
      Promise.resolve(
        prompt.some((message) => message.role === "tool")
          ? "알겠어요."
          : { toolName: "computer_click", input: { window: "w99", x: 1, y: 1, intent: "누르기" } },
      );

    await send(token, agentId, "눌러줘");
    await waitForIdle(agentId);
    const result = JSON.stringify(
      llm.prompts.at(-1)?.findLast((message) => message.role === "tool"),
    );
    expect(result).toContain("Look again with computer_view");
  });

  test("without a Mac, looking says so", async () => {
    const { token } = await signInDevice("minho@example.com", "iPhone", "ios");
    const agentId = await createAgent(token);
    llm.reply = (prompt) =>
      Promise.resolve(
        prompt.some((message) => message.role === "tool")
          ? "맥이 없어요."
          : { toolName: "computer_view", input: {} },
      );

    await send(token, agentId, "맥 화면 봐줘");
    await waitForIdle(agentId);
    const result = JSON.stringify(
      llm.prompts.at(-1)?.findLast((message) => message.role === "tool"),
    );
    expect(result).toContain("No computers are connected");
  });
});

import type { ModelMessage, ToolResultPart } from "ai";
import { and, eq, sql } from "drizzle-orm";
import type { z } from "zod";
import type { Transaction } from "../../db";
import { messages } from "../../db/schema/agents";
import * as devices from "../devices";
import {
  ComputerOutcome,
  ComputerScreen,
  ComputerStatus,
  type ComputerTarget,
  ComputerView,
  methods,
} from "./protocol";

const STATUS_TIMEOUT_MS = 2_000;
const ACTION_TIMEOUT_MS = 30_000;
const TOOL_PREFIX = "computer_";
const ID = /\[([ew])(\d+)\]/g;

type Computer = { id: string; name: string; isOnline: boolean; isAvailable: boolean };
type Ref = { deviceId: string; kind: "window" | "element"; ref: string };

/** What a computer tool hands the model: text first, then the image the text describes. */
export type Seen = { text: string; image?: { data: string; mediaType: "image/jpeg" } };

/** A point on a window's image, as the model gives it. */
export type Point = { window: string; x: number; y: number };

export type Action =
  | {
      method: "click";
      target: string | Point;
      button?: string;
      count?: number;
      modifiers?: string[];
    }
  | { method: "hover"; target: string | Point }
  | { method: "drag"; from: string | Point; to: string | Point; modifiers?: string[] }
  | { method: "scroll"; target: string | Point; direction: string; amount: number }
  | { method: "type"; target: string | Point; text: string }
  | { method: "key"; target: string | Point; keys: string; repeat?: number }
  | { method: "set"; target: string; value: string }
  | { method: "menu"; window: string; path: string[] };

export class UnknownId extends Error {}

/**
 * One run's view of the user's computers. It turns the computers' own window and element refs
 * into the short ids the model sees (w3, e12), so an id alone says which computer it belongs to.
 */
export function openComputers(userId: string, history: readonly ModelMessage[]) {
  const ids = new Map<string, Ref>();
  const refs = new Map<string, string>();
  // Ids already in the conversation keep their meaning; new ones start after them.
  let next = highestId(history) + 1;

  async function look(input: {
    computer?: string;
    window?: string;
    region?: { x: number; y: number; width: number; height: number };
    wait?: number;
  }): Promise<Seen> {
    if (input.window) {
      const window = resolve(input.window, "window");
      const computer = await computerOf(window.deviceId);
      const view = ComputerView.parse(
        await callOn(computer, methods.view, {
          window: window.ref,
          region: input.region,
          waitSeconds: input.wait,
        }),
      );
      return renderView(computer, view);
    }

    if (input.wait) await Bun.sleep(Math.min(input.wait, 60) * 1000);
    const computers = await list();
    if (computers.length === 0) {
      return {
        text: "No computers are connected. When the user installs Goat for Mac and signs in, you can use their Mac.",
      };
    }
    const roster = computers.map(describe).join("\n");
    const chosen = input.computer
      ? computers.find((computer) => computer.id === input.computer)
      : undefined;
    if (input.computer && !chosen) {
      throw new UnknownId(`No computer has the id ${input.computer}.\nComputers:\n${roster}`);
    }

    const available = computers.filter((computer) => computer.isAvailable);
    const shown = chosen ? [chosen] : available;
    if (shown.length === 0) return { text: `Computers:\n${roster}\nNone of them can be used now.` };
    if (chosen && !chosen.isAvailable) {
      return { text: `Computers:\n${roster}\n${chosen.name} cannot be used now.` };
    }

    const withImage = shown.length === 1;
    const screens = await Promise.all(
      shown.map(async (computer) => ({
        computer,
        screen: ComputerScreen.parse(
          await callOn(computer, methods.screen, { includeImage: withImage }),
        ),
      })),
    );
    const text = [
      `Computers:\n${roster}`,
      ...screens.map(({ computer, screen }) => renderScreen(computer, screen)),
      ...(withImage ? [] : ["Pass a computer id to see its screen."]),
    ].join("\n\n");
    const image = withImage ? screens[0]?.screen.image : undefined;
    return image ? { text, image: { data: image.data, mediaType: "image/jpeg" } } : { text };
  }

  async function act(action: Action): Promise<Seen> {
    const deviceIds = new Set<string>();
    const target = (id: string | Point): ComputerTarget => {
      const resolved = resolveTarget(id);
      deviceIds.add(resolved.deviceId);
      return resolved.target;
    };
    const params = paramsOf(action, target, (id) => {
      const window = resolve(id, "window");
      deviceIds.add(window.deviceId);
      return window.ref;
    });
    const [deviceId, ...others] = deviceIds;
    if (!deviceId || others.length > 0) throw new UnknownId("Targets must be on one computer.");

    const computer = await computerOf(deviceId);
    const outcome = ComputerOutcome.parse(await callOn(computer, methods[action.method], params));
    return renderOutcome(computer, outcome);
  }

  async function open(input: {
    computer?: string;
    app?: string;
    path?: string;
    url?: string;
  }): Promise<Seen> {
    const computer = await pick(input.computer);
    const params = { app: input.app, path: input.path, url: input.url };
    const outcome = ComputerOutcome.parse(await callOn(computer, methods.open, params));
    return renderOutcome(computer, outcome);
  }

  async function clipboard(input: {
    computer?: string;
    text?: string;
    path?: string;
  }): Promise<Seen> {
    const computer = await pick(input.computer);
    const params = { text: input.text, path: input.path };
    const outcome = ComputerOutcome.parse(await callOn(computer, methods.clipboard, params));
    if (!outcome.clipboard) return { text: "Copied." };
    const { text, files } = outcome.clipboard;
    const lines = [
      ...(text === undefined ? [] : [`Text:\n${text}`]),
      ...(files?.length ? [`Files:\n${files.join("\n")}`] : []),
    ];
    return { text: lines.length > 0 ? lines.join("\n\n") : "The clipboard is empty." };
  }

  async function list(): Promise<Computer[]> {
    const macs = (await devices.listDevices(userId)).filter(
      (device) => device.platform === "macos",
    );
    return Promise.all(
      macs.map(async ({ id, name }) => {
        try {
          const status = ComputerStatus.parse(
            await devices.call(id, methods.status, {}, { timeoutMs: STATUS_TIMEOUT_MS }),
          );
          return { id, name, isOnline: true, isAvailable: status.isAvailable };
        } catch {
          // A computer that does not answer is listed as offline.
          return { id, name, isOnline: false, isAvailable: false };
        }
      }),
    );
  }

  async function computerOf(deviceId: string): Promise<Computer> {
    const device = (await devices.listDevices(userId)).find(({ id }) => id === deviceId);
    if (!device) throw new UnknownId("That computer was removed from the account.");
    return { id: device.id, name: device.name, isOnline: true, isAvailable: true };
  }

  async function pick(wanted: string | undefined): Promise<Computer> {
    const computers = await list();
    const roster = computers.map(describe).join("\n");
    if (wanted) {
      const computer = computers.find(({ id }) => id === wanted);
      if (!computer)
        throw new UnknownId(`No computer has the id ${wanted}.\nComputers:\n${roster}`);
      return computer;
    }
    const available = computers.filter((computer) => computer.isAvailable);
    if (available.length === 1 && available[0]) return available[0];
    if (available.length === 0) {
      throw new UnknownId(
        computers.length === 0
          ? "No computers are connected."
          : `No computer can be used now.\nComputers:\n${roster}`,
      );
    }
    throw new UnknownId(`Say which computer.\nComputers:\n${roster}`);
  }

  function resolveTarget(id: string | Point): { deviceId: string; target: ComputerTarget } {
    if (typeof id !== "string") {
      const window = resolve(id.window, "window");
      return { deviceId: window.deviceId, target: { window: window.ref, x: id.x, y: id.y } };
    }
    const resolved = resolve(id);
    return {
      deviceId: resolved.deviceId,
      target: resolved.kind === "window" ? { window: resolved.ref } : { element: resolved.ref },
    };
  }

  function resolve(id: string, kind?: Ref["kind"]): Ref {
    const ref = ids.get(id);
    if (!ref || (kind && ref.kind !== kind)) {
      throw new UnknownId(
        `${id} is not ${kind === "window" ? "a window" : "an element"} on a screen you looked at in this task. Look again with computer_view.`,
      );
    }
    return ref;
  }

  function idFor(deviceId: string, kind: Ref["kind"], ref: string): string {
    const key = `${deviceId} ${kind} ${ref}`;
    const known = refs.get(key);
    if (known) return known;
    const id = `${kind === "window" ? "w" : "e"}${next++}`;
    refs.set(key, id);
    ids.set(id, { deviceId, kind, ref });
    return id;
  }

  function renderScreen(computer: Computer, screen: z.infer<typeof ComputerScreen>): string {
    const screenId = idFor(computer.id, "window", screen.ref);
    const size = screen.image ? ` (${screen.image.width}×${screen.image.height})` : "";
    const windows = screen.windows.map((window) => {
      const notes = [window.isFront ? "front" : "", window.isMinimized ? "minimized" : ""]
        .filter(Boolean)
        .join(", ");
      const id = idFor(computer.id, "window", window.ref);
      return `- ${window.app} — ${JSON.stringify(window.title)} [${id}]${notes ? ` (${notes})` : ""}`;
    });
    return [
      `${computer.name} · screen [${screenId}]${size}`,
      "Windows, front to back:",
      ...(windows.length > 0 ? windows : ["(none)"]),
    ].join("\n");
  }

  function renderView(computer: Computer, view: z.infer<typeof ComputerView>): Seen {
    const windowId = idFor(computer.id, "window", view.window.ref);
    const header = `${computer.name} · ${view.window.app} — ${JSON.stringify(view.window.title)} [${windowId}] · image ${view.image.width}×${view.image.height}`;
    const lines = view.nodes.map((node) => {
      const parts = [`${"  ".repeat(node.depth)}- ${node.role}`];
      if (node.label) parts.push(JSON.stringify(node.label));
      if (node.value) parts.push(`value=${JSON.stringify(node.value)}`);
      if (node.states?.length) parts.push(node.states.join(" "));
      if (node.ref) parts.push(`[${idFor(computer.id, "element", node.ref)}]`);
      return parts.join(" ");
    });
    if (view.isTruncated) lines.push("(more elements left out; zoom or scroll to see them)");
    return {
      text: [header, ...lines].join("\n"),
      image: { data: view.image.data, mediaType: "image/jpeg" },
    };
  }

  function renderOutcome(computer: Computer, outcome: z.infer<typeof ComputerOutcome>): Seen {
    const note = outcome.isStale
      ? "The element changed before it could be used, so nothing was done. The window now:"
      : "Done.";
    if (!outcome.view) return { text: note };
    const seen = renderView(computer, outcome.view);
    return { ...seen, text: `${note}\n\n${seen.text}` };
  }

  return { look, act, open, clipboard };
}

export type Computers = ReturnType<typeof openComputers>;

async function callOn(computer: Computer, method: string, params: unknown): Promise<unknown> {
  try {
    return await devices.call(computer.id, method, params, { timeoutMs: ACTION_TIMEOUT_MS });
  } catch (error) {
    if (error instanceof devices.DeviceOffline) {
      throw new Error(`${computer.name} is offline.`, { cause: error });
    }
    if (error instanceof devices.DeviceStoppedResponding) {
      throw new Error(
        `${computer.name} stopped responding. The action may or may not have happened; look before trying again.`,
        { cause: error },
      );
    }
    throw error;
  }
}

function paramsOf(
  action: Action,
  target: (id: string | Point) => ComputerTarget,
  window: (id: string) => string,
): object {
  let params: object;
  switch (action.method) {
    case "click":
      params = {
        target: target(action.target),
        button: action.button,
        count: action.count,
        modifiers: action.modifiers,
      };
      break;
    case "hover":
      params = { target: target(action.target) };
      break;
    case "drag":
      params = { from: target(action.from), to: target(action.to), modifiers: action.modifiers };
      break;
    case "scroll":
      params = {
        target: target(action.target),
        direction: action.direction,
        amount: action.amount,
      };
      break;
    case "type":
      params = { target: target(action.target), text: action.text };
      break;
    case "key":
      params = { target: target(action.target), keys: action.keys, repeat: action.repeat };
      break;
    case "set":
      params = { target: target(action.target), value: action.value };
      break;
    case "menu":
      params = { window: window(action.window), path: action.path };
      break;
  }
  return params;
}

function describe(computer: Computer): string {
  const state = !computer.isOnline
    ? "offline"
    : computer.isAvailable
      ? "online"
      : "online, but the user has not allowed its use";
  return `- ${JSON.stringify(computer.name)} id ${computer.id} (${state})`;
}

function highestId(history: readonly ModelMessage[]): number {
  let highest = 0;
  for (const message of history) {
    if (message.role !== "tool") continue;
    for (const part of message.content) {
      if (part.type !== "tool-result" || !part.toolName.startsWith(TOOL_PREFIX)) continue;
      for (const match of JSON.stringify(part.output).matchAll(ID)) {
        highest = Math.max(highest, Number(match[2]));
      }
    }
  }
  return highest;
}

/** The text and image the model gets from a computer tool. */
export function toModelOutput(seen: Seen): ToolResultPart["output"] {
  if (!seen.image) return { type: "text", value: seen.text };
  return {
    type: "content",
    value: [
      { type: "text", text: seen.text },
      {
        type: "file",
        data: { type: "data", data: seen.image.data },
        mediaType: seen.image.mediaType,
      },
    ],
  };
}

/**
 * Shrinks the screens of finished work to their first line. They cannot be looked at again, and
 * the conversation never ends, so they would otherwise pile up in every later request.
 */
export async function compactScreens(tx: Transaction, agentId: string): Promise<void> {
  const rows = await tx
    .select({ id: messages.id, message: messages.message })
    .from(messages)
    .where(
      and(
        eq(messages.agentId, agentId),
        sql`jsonb_path_exists(${messages.message}, '$.content[*] ? (@.toolName starts with "computer_" && @.output.type == "content")')`,
      ),
    );
  for (const { id, message } of rows) {
    if (message.role !== "tool") continue;
    const content = message.content.map((part) =>
      part.type === "tool-result" &&
      part.toolName.startsWith(TOOL_PREFIX) &&
      part.output.type === "content"
        ? { ...part, output: compacted(part.output.value) }
        : part,
    );
    await tx
      .update(messages)
      .set({ message: { ...message, content } })
      .where(eq(messages.id, id));
  }
}

function compacted(parts: { type: string; text?: string }[]): ToolResultPart["output"] {
  const text = parts.find((part) => part.type === "text")?.text ?? "";
  const lines = text.split("\n");
  const header = lines.find((line) => line.includes(" · ")) ?? lines[0] ?? "A screen";
  return { type: "text", value: `${header} (screen removed after the task)` };
}

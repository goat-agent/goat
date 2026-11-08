import type { ModelMessage, ToolResultPart } from "ai";
import { and, eq, sql } from "drizzle-orm";
import type { z } from "zod";
import type { Transaction } from "../../db";
import { messages } from "../../db/schema/agents";
import * as devices from "../devices";
import {
  type ActionMethod,
  ComputerOutcome,
  ComputerStatus,
  ComputerView,
  ComputerWindows,
  methods,
  type params,
} from "./protocol";

const STATUS_TIMEOUT_MS = 2_000;
const ACTION_TIMEOUT_MS = 30_000;
const TOOL_PREFIX = "computer_";
const ID = /\[w(\d+)\]/g;

type Computer = { id: string; name: string; isOnline: boolean; isAvailable: boolean };
type WindowRef = { deviceId: string; ref: string; app: string };

/** What a computer tool hands the model: text first, then the image the text describes. */
export type Seen = { text: string; image?: { data: string; mediaType: "image/jpeg" } };

/** An action as the model gives it: `window` is a short id such as w3. */
export type ActionInput<M extends ActionMethod> = z.infer<(typeof params)[M]>;

export class UnknownId extends Error {}

/**
 * One run's view of the user's computers. It turns each computer's own window refs into the short
 * ids the model sees (w3), so an id alone says which computer it belongs to.
 */
export function openComputers(userId: string, floor: number) {
  const ids = new Map<string, WindowRef>();
  const refs = new Map<string, string>();
  // Ids already in the conversation keep their meaning; new ones start after them.
  let next = floor + 1;

  async function look(input: {
    computer?: string;
    window?: string;
    region?: { x: number; y: number; width: number; height: number };
    wait?: number;
  }): Promise<Seen> {
    if (input.window) {
      const window = resolve(input.window);
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
    const shown = chosen ? [chosen] : computers.filter((computer) => computer.isAvailable);
    if (shown.length === 0 || (chosen && !chosen.isAvailable)) {
      return { text: `Computers:\n${roster}\nNone of them can be used now.` };
    }
    const sections = await Promise.all(
      shown.map(async (computer) =>
        renderWindows(computer, ComputerWindows.parse(await callOn(computer, methods.windows, {}))),
      ),
    );
    return { text: [`Computers:\n${roster}`, ...sections].join("\n\n") };
  }

  async function act<M extends ActionMethod>(method: M, input: ActionInput<M>): Promise<Seen> {
    const window = resolve(input.window);
    const computer = await computerOf(window.deviceId);
    const outcome = ComputerOutcome.parse(
      await callOn(computer, methods[method], { ...input, window: window.ref }),
    );
    return renderOutcome(computer, outcome, window.app);
  }

  async function open(input: {
    computer?: string;
    app?: string;
    path?: string;
    url?: string;
    foreground?: boolean;
  }): Promise<Seen> {
    const computer = await pick(input.computer);
    const { computer: _computer, ...openParams } = input;
    const outcome = ComputerOutcome.parse(await callOn(computer, methods.open, openParams));
    return renderOutcome(computer, outcome, input.app ?? "the app");
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
      if (!computer) {
        throw new UnknownId(`No computer has the id ${wanted}.\nComputers:\n${roster}`);
      }
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

  function resolve(id: string): WindowRef {
    const window = ids.get(id);
    if (!window) {
      throw new UnknownId(
        `${id} is not a window you looked at in this task. Look again with computer_view.`,
      );
    }
    return window;
  }

  function idFor(deviceId: string, window: { ref: string; app: string }): string {
    const key = `${deviceId} ${window.ref}`;
    const known = refs.get(key);
    if (known) return known;
    const id = `w${next++}`;
    refs.set(key, id);
    ids.set(id, { deviceId, ref: window.ref, app: window.app });
    return id;
  }

  function renderWindows(computer: Computer, { windows }: z.infer<typeof ComputerWindows>): string {
    const lines = windows.map((window) => {
      const notes = [window.isFront ? "front" : "", window.isMinimized ? "minimized" : ""]
        .filter(Boolean)
        .join(", ");
      const id = idFor(computer.id, window);
      return `- ${window.app} — ${JSON.stringify(window.title)} [${id}]${notes ? ` (${notes})` : ""}`;
    });
    return [
      `${computer.name} · windows, front to back:`,
      ...(lines.length > 0 ? lines : ["(none)"]),
    ].join("\n");
  }

  function renderView(computer: Computer, view: z.infer<typeof ComputerView>): Seen {
    const id = idFor(computer.id, view.window);
    return {
      text: `${computer.name} · ${view.window.app} — ${JSON.stringify(view.window.title)} [${id}] · image ${view.image.width}×${view.image.height}`,
      image: { data: view.image.data, mediaType: "image/jpeg" },
    };
  }

  function renderOutcome(computer: Computer, outcome: ComputerOutcome, app: string): Seen {
    const note = noteOf(outcome, outcome.view?.window.app ?? app);
    if (!outcome.view) return { text: note };
    const seen = renderView(computer, outcome.view);
    return { ...seen, text: `${note}\n\n${seen.text}` };
  }

  return { look, act, open };
}

export type Computers = ReturnType<typeof openComputers>;

function noteOf(outcome: ComputerOutcome, app: string): string {
  switch (outcome.notDone) {
    case undefined:
      return outcome.delivery === "foreground"
        ? `Done. ${app} was brought to the front for it, with the user's permission.`
        : "Done.";
    case "needs_foreground":
      return `Not done: ${app} does not take this while it is in the background. Call again with foreground: true; the user is asked before ${app} comes to the front.`;
    case "declined":
      return `Not done: the user did not let ${app} come to the front. Ask them in the conversation before trying again.`;
    case "minimized":
      return "Not done: the window is minimized. Call again with foreground: true to restore it; the user is asked first.";
    default:
      return `Not done (${outcome.notDone}).`;
  }
}

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

function describe(computer: Computer): string {
  const state = !computer.isOnline
    ? "offline"
    : computer.isAvailable
      ? "online"
      : "online, but the user has not allowed its use";
  return `- ${JSON.stringify(computer.name)} id ${computer.id} (${state})`;
}

export function highestId(history: readonly ModelMessage[]): number {
  let highest = 0;
  for (const message of history) {
    if (message.role !== "tool") continue;
    for (const part of message.content) {
      if (part.type !== "tool-result" || !part.toolName.startsWith(TOOL_PREFIX)) continue;
      for (const match of JSON.stringify(part.output).matchAll(ID)) {
        highest = Math.max(highest, Number(match[1]));
      }
    }
  }
  return highest;
}

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

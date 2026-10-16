import { expect, test } from "bun:test";
import type { ScreenEvent } from "./event.ts";
import type { Frame, Key, Terminal, TerminalEvent } from "@goat/terminal";
import type { UserMessage } from "@goat/sdk/provider";
import { type Engine, runTui } from "./app.ts";

interface Channel<T> {
  readonly iterable: AsyncIterable<T>;
  push(...items: readonly T[]): void;
}

function channel<T extends Readonly<object>>(): Channel<T> {
  const items: T[] = [];
  const waiting: ((item: T) => void)[] = [];
  const nextPushed = async (): Promise<T> =>
    new Promise((resolve) => {
      waiting.push(resolve);
    });
  return {
    iterable: {
      [Symbol.asyncIterator]: () => ({
        next: async () => ({ value: items.shift() ?? (await nextPushed()), done: false }),
      }),
    },
    push: (...pushed) => {
      for (const item of pushed) {
        const resolve = waiting.shift();
        if (resolve === undefined) {
          items.push(item);
        } else {
          resolve(item);
        }
      }
    },
  };
}

const none = { shift: false, alt: false, ctrl: false, meta: false };

function key(name: string, mods: Partial<Key> = {}): TerminalEvent {
  return { type: "key", key: { name, ...none, ...mods } };
}

const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });

function typed(text: string): readonly TerminalEvent[] {
  const events: TerminalEvent[] = [];
  for (const { segment } of segmenter.segment(text)) {
    events.push(key(segment, { text: segment }));
  }
  return events;
}

interface Harness {
  readonly input: Channel<TerminalEvent>;
  readonly session: Channel<ScreenEvent>;
  readonly submitted: readonly UserMessage[];
  readonly stops: readonly number[];
  readonly frames: readonly Frame[];
  readonly done: Promise<void>;
}

function start(): Harness {
  const input = channel<TerminalEvent>();
  const session = channel<ScreenEvent>();
  const submitted: UserMessage[] = [];
  const stops: number[] = [];
  const frames: Frame[] = [];
  const terminal: Terminal = {
    capabilities: { kittyKeyboard: false },
    events: input.iterable,
    size: () => ({ columns: 30, rows: 8 }),
    draw: (frame) => {
      frames.push(frame);
    },
    close: () => {
      frames.length = 0;
    },
  };
  const engine: Engine = {
    events: session.iterable,
    submit: (message) => {
      submitted.push(message);
      session.push({ type: "user", message });
      return true;
    },
    stop: () => {
      stops.push(stops.length);
    },
  };
  return { input, session, submitted, stops, frames, done: runTui(terminal, engine) };
}

function screen(harness: Harness): string {
  return (harness.frames.at(-1)?.lines ?? []).map((line) => Bun.stripANSI(line)).join("\n");
}

async function settle(): Promise<void> {
  await Bun.sleep(40);
}

test("submits the typed message and shows it in the transcript", async () => {
  const harness = start();
  harness.input.push(...typed("hello"), key("enter"));
  await settle();
  expect(harness.submitted).toEqual([{ role: "user", parts: [{ type: "text", text: "hello" }] }]);
  expect(screen(harness)).toContain("> hello");
  expect(screen(harness)).toContain(`│${" ".repeat(28)}│`);
  harness.input.push(key("c", { ctrl: true }), key("escape"));
  await settle();
  expect(harness.stops).toHaveLength(2);
});

test("keeps the draft when Enter is pressed during a turn", async () => {
  const harness = start();
  harness.input.push(...typed("one"), key("enter"), ...typed("two"), key("enter"));
  await settle();
  expect(harness.submitted).toHaveLength(1);
  expect(screen(harness)).toContain("│ two");
});

test("asks for a second Ctrl+C before exiting", async () => {
  const harness = start();
  harness.input.push(key("c", { ctrl: true }));
  await settle();
  expect(screen(harness)).toContain("Press Ctrl+C again to exit");
  harness.input.push(key("c", { ctrl: true }));
  await harness.done;
});

test("exits on Ctrl+D with an empty composer", async () => {
  const harness = start();
  harness.input.push(key("d", { ctrl: true }));
  await harness.done;
});

test("starts without an engine and keeps the draft on Enter", async () => {
  const input = channel<TerminalEvent>();
  const frames: Frame[] = [];
  const terminal: Terminal = {
    capabilities: { kittyKeyboard: false },
    events: input.iterable,
    size: () => ({ columns: 30, rows: 8 }),
    draw: (frame) => {
      frames.push(frame);
    },
    close: () => {
      frames.length = 0;
    },
  };
  const done = runTui(terminal);
  input.push(...typed("hi"), key("enter"));
  await settle();
  const shown = (frames.at(-1)?.lines ?? []).map((line) => Bun.stripANSI(line)).join("\n");
  expect(shown).toContain("│ hi");
  input.push(key("u", { ctrl: true }), key("d", { ctrl: true }));
  await done;
});

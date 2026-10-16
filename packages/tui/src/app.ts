import type { ScreenEvent } from "./event.ts";
import type { Key, Terminal, TerminalEvent } from "@goat/terminal";
import type { UserMessage } from "@goat/provider";
import * as edit from "./composer.ts";
import { type Command, commandFor } from "./keymap.ts";
import { layout, type Placement } from "./layout.ts";
import { createRenderer } from "./render.ts";
import { emptyTranscript, reduce, type Transcript } from "./transcript.ts";

export interface Engine {
  readonly events: AsyncIterable<ScreenEvent>;
  submit(message: UserMessage): boolean;
  stop(): void;
}

interface State {
  readonly transcript: Transcript;
  readonly composer: edit.Composer;
  readonly top: number | undefined;
  readonly hint: string;
  readonly exitArmed: boolean;
}

type Outcome = "exit" | undefined;

const frameDelay = 16;
const exitWindow = 1_500;
const wheelLines = 3;

const edits: Partial<Readonly<Record<Command, (composer: edit.Composer) => edit.Composer>>> = {
  newline: (composer) => edit.insert(composer, "\n"),
  "delete-back": (composer) => edit.backspace(composer),
  "delete-forward": (composer) => edit.deleteForward(composer),
  "delete-word-back": (composer) => edit.deleteWordBack(composer),
  "delete-line-start": (composer) => edit.deleteToLineStart(composer),
  "delete-line-end": (composer) => edit.deleteToLineEnd(composer),
  left: (composer) => edit.moveLeft(composer),
  right: (composer) => edit.moveRight(composer),
  "word-left": (composer) => edit.moveWordLeft(composer),
  "word-right": (composer) => edit.moveWordRight(composer),
  up: (composer) => edit.moveUp(composer),
  down: (composer) => edit.moveDown(composer),
  "line-start": (composer) => edit.moveLineStart(composer),
  "line-end": (composer) => edit.moveLineEnd(composer),
};

export async function runTui(terminal: Terminal, engine?: Engine): Promise<void> {
  await new Tui(terminal, engine).run();
}

class Tui {
  readonly #terminal: Terminal;
  readonly #engine: Engine | undefined;
  readonly #renderer = createRenderer();
  #state: State = {
    transcript: emptyTranscript,
    composer: edit.emptyComposer,
    top: undefined,
    hint: "",
    exitArmed: false,
  };
  #placement: Placement = { top: 0, height: 0, total: 0 };
  #frameTimer: ReturnType<typeof setTimeout> | undefined;
  #exitTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(terminal: Terminal, engine: Engine | undefined) {
    this.#terminal = terminal;
    this.#engine = engine;
  }

  async run(): Promise<void> {
    if (this.#engine !== undefined) {
      void this.#follow(this.#engine);
    }
    this.#draw();
    for await (const event of this.#terminal.events) {
      if (this.#handle(event) === "exit") {
        break;
      }
    }
    clearTimeout(this.#frameTimer);
    clearTimeout(this.#exitTimer);
  }

  async #follow(engine: Engine): Promise<void> {
    for await (const event of engine.events) {
      this.#set({ ...this.#state, transcript: reduce(this.#state.transcript, event) });
    }
  }

  #draw(): void {
    this.#frameTimer = undefined;
    const { transcript, composer, top, hint } = this.#state;
    const result = layout(
      { blocks: transcript.blocks, composer, top, hint },
      this.#terminal.size(),
      this.#renderer,
    );
    this.#placement = result.placement;
    this.#terminal.draw(result.frame);
  }

  #set(next: State): void {
    if (next === this.#state) {
      return;
    }
    this.#state = next;
    this.#frameTimer ??= setTimeout(() => {
      this.#draw();
    }, frameDelay);
  }

  #handle(event: TerminalEvent): Outcome {
    if (event.type === "key") {
      return this.#key(event.key);
    }
    if (event.type === "paste") {
      this.#set({ ...this.#state, composer: edit.paste(this.#state.composer, event.text) });
    } else if (event.type === "mouse") {
      const { mouse } = event;
      if (mouse.action === "wheel" && (mouse.direction === "up" || mouse.direction === "down")) {
        this.#scroll(mouse.direction === "up" ? -wheelLines : wheelLines);
      }
    } else {
      this.#draw();
    }
    return undefined;
  }

  #key(key: Key): Outcome {
    const command = commandFor(key);
    if (command === undefined) {
      if (key.text !== undefined && !key.ctrl && !key.alt && !key.meta) {
        this.#set({ ...this.#state, composer: edit.insert(this.#state.composer, key.text) });
      }
      return undefined;
    }
    const change = edits[command];
    if (change !== undefined) {
      this.#set({ ...this.#state, composer: change(this.#state.composer) });
      return undefined;
    }
    return this.#control(command);
  }

  #control(command: Command): Outcome {
    const page = Math.max(1, this.#placement.height - 1);
    if (command === "submit") {
      this.#submit();
    } else if (command === "stop") {
      this.#stop();
    } else if (command === "interrupt") {
      return this.#interrupt();
    } else if (command === "exit-if-empty") {
      return this.#state.composer.text === "" ? "exit" : undefined;
    } else if (command === "page-up" || command === "page-down") {
      this.#scroll(command === "page-up" ? -page : page);
    }
    return undefined;
  }

  #submit(): void {
    const text = edit.expand(this.#state.composer);
    if (this.#state.transcript.running || text.trim() === "") {
      return;
    }
    if (this.#engine?.submit({ role: "user", parts: [{ type: "text", text }] }) === true) {
      this.#set({ ...this.#state, composer: edit.emptyComposer, top: undefined });
    }
  }

  #stop(): void {
    if (this.#state.transcript.running) {
      this.#engine?.stop();
    }
  }

  #interrupt(): Outcome {
    if (this.#state.transcript.running) {
      this.#engine?.stop();
      return undefined;
    }
    if (this.#state.composer.text !== "") {
      this.#set({ ...this.#state, composer: edit.emptyComposer });
      return undefined;
    }
    if (this.#state.exitArmed) {
      return "exit";
    }
    clearTimeout(this.#exitTimer);
    this.#exitTimer = setTimeout(() => {
      this.#set({ ...this.#state, exitArmed: false, hint: "" });
    }, exitWindow);
    this.#set({ ...this.#state, exitArmed: true, hint: "Press Ctrl+C again to exit" });
    return undefined;
  }

  #scroll(lines: number): void {
    const { top, height, total } = this.#placement;
    const next = Math.max(0, top + lines);
    this.#set({ ...this.#state, top: next + height >= total ? undefined : next });
  }
}

import type { ScreenEvent } from "./event.ts";
import type { AgentPart, MediaPart, Stop, TextPart, ToolResultPart } from "@goat/provider";

interface Base {
  readonly id: number;
  readonly version: number;
}

export type Block = Base &
  (
    | { readonly kind: "user"; readonly text: string }
    | { readonly kind: "text"; readonly text: string; readonly streaming: boolean }
    | { readonly kind: "reasoning"; readonly text: string; readonly streaming: boolean }
    | {
        readonly kind: "tool";
        readonly callId: string;
        readonly name: string;
        readonly input: string;
        readonly streaming: boolean;
        readonly result?: { readonly text: string; readonly isError: boolean };
      }
    | { readonly kind: "notice"; readonly text: string; readonly tone: "dim" | "error" }
  );

export interface Transcript {
  readonly blocks: readonly Block[];
  readonly open: ReadonlyMap<number, number>;
  readonly running: boolean;
  readonly nextId: number;
}

export const emptyTranscript: Transcript = {
  blocks: [],
  open: new Map(),
  running: false,
  nextId: 1,
};

export function reduce(state: Transcript, event: ScreenEvent): Transcript {
  if (event.type === "user") {
    return { ...append(state, { kind: "user", text: textOf(event.message.parts) }), running: true };
  }
  if (event.type === "part_start") {
    const block = startBlock(event.part);
    return block === undefined
      ? state
      : {
          ...append(state, block),
          open: new Map(state.open).set(event.index, state.blocks.length),
        };
  }
  if (event.type === "part_delta") {
    return update(state, event.index, (block) => grow(block, event.text));
  }
  if (event.type === "part_end") {
    return update(state, event.index, (block) => finish(block, event.part));
  }
  if (event.type === "message") {
    return { ...state, open: new Map() };
  }
  if (event.type === "tool_result") {
    return attachResult(state, event.result);
  }
  const ended = { ...state, running: false, open: new Map<number, number>() };
  const notice = noticeOf(event.stop);
  return notice === undefined ? ended : append(ended, notice);
}

function attachResult(state: Transcript, result: ToolResultPart): Transcript {
  const position = state.blocks.findLastIndex(
    (block) => block.kind === "tool" && block.callId === result.callId,
  );
  const block = state.blocks[position];
  if (block?.kind !== "tool") {
    return state;
  }
  return replace(state, position, {
    ...block,
    version: block.version + 1,
    result: { text: textOf(result.parts), isError: result.isError },
  });
}

type Without<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
type Draft = Without<Block, "id" | "version">;

function append(state: Transcript, draft: Draft): Transcript {
  const block: Block = { ...draft, id: state.nextId, version: 0 };
  return { ...state, blocks: [...state.blocks, block], nextId: state.nextId + 1 };
}

function replace(state: Transcript, position: number, block: Block): Transcript {
  return { ...state, blocks: state.blocks.with(position, block) };
}

function update(state: Transcript, index: number, change: (block: Block) => Block): Transcript {
  const position = state.open.get(index);
  const block = position === undefined ? undefined : state.blocks[position];
  return position === undefined || block === undefined
    ? state
    : replace(state, position, change(block));
}

function startBlock(part: AgentPart): Draft | undefined {
  if (part.type === "text" || part.type === "reasoning") {
    return { kind: part.type, text: part.text, streaming: true };
  }
  if (part.type === "opaque") {
    return undefined;
  }
  return { kind: "tool", callId: part.id, name: part.name, input: part.input, streaming: true };
}

function grow(block: Block, text: string): Block {
  const version = block.version + 1;
  if (block.kind === "text" || block.kind === "reasoning") {
    return { ...block, version, text: block.text + text };
  }
  return block.kind === "tool" ? { ...block, version, input: block.input + text } : block;
}

function finish(block: Block, part: AgentPart): Block {
  const version = block.version + 1;
  if ((block.kind === "text" || block.kind === "reasoning") && part.type === block.kind) {
    return { ...block, version, text: part.text, streaming: false };
  }
  if (block.kind === "tool" && part.type === "tool_call") {
    return { ...block, version, input: part.input, streaming: false };
  }
  return block;
}

const notices: Readonly<Record<Exclude<Stop["reason"], "error">, Draft | undefined>> = {
  done: undefined,
  aborted: { kind: "notice", text: "Stopped.", tone: "dim" },
  length: { kind: "notice", text: "The reply reached the output limit.", tone: "dim" },
  overflow: { kind: "notice", text: "The conversation is too long for this model.", tone: "error" },
  refusal: { kind: "notice", text: "The model refused to answer.", tone: "error" },
};

function noticeOf(stop: Stop): Draft | undefined {
  return stop.reason === "error"
    ? { kind: "notice", text: stop.error.message, tone: "error" }
    : notices[stop.reason];
}

function textOf(parts: readonly (TextPart | MediaPart)[]): string {
  return parts.map((part) => (part.type === "text" ? part.text : "[Image]")).join("");
}

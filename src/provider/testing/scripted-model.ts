import type { Event } from "../event.ts";
import type { AgentMessage, AgentPart } from "../message.ts";
import type { Model, ModelInfo } from "../model.ts";
import { ok } from "../result.ts";

export interface Script {
  readonly provider: string;
  readonly info: ModelInfo;
  readonly replies: readonly AgentMessage[];
}

export function scriptedModel(script: Script): Model {
  let turn = 0;
  return {
    id: script.info.id,
    provider: script.provider,
    info: () => Promise.resolve(ok(script.info)),
    stream: (_input, options) => {
      const reply = script.replies[turn] ?? missingReply(script);
      turn += 1;
      return asyncOf(play(reply, options?.signal));
    },
  };
}

function* play(reply: AgentMessage, signal: AbortSignal | undefined): Generator<Event> {
  for (const [index, part] of reply.parts.entries()) {
    if (signal?.aborted === true) {
      yield {
        type: "end",
        message: { ...reply, parts: reply.parts.slice(0, index), stop: { reason: "aborted" } },
      };
      return;
    }
    yield { type: "part_start", index, part: emptied(part) };
    const text = textOf(part);
    if (text.length > 0) {
      yield { type: "part_delta", index, text };
    }
    yield { type: "part_end", index, part };
  }
  yield { type: "end", message: reply };
}

function asyncOf<T>(items: Iterable<T>): AsyncIterable<T> {
  return {
    [Symbol.asyncIterator]: () => {
      const iterator = items[Symbol.iterator]();
      return { next: () => Promise.resolve(iterator.next()) };
    },
  };
}

function missingReply(script: Script): AgentMessage {
  return {
    role: "agent",
    producer: { provider: script.provider, model: script.info.id },
    parts: [],
    stop: {
      reason: "error",
      error: { kind: "invalid", message: "The scripted model has no reply for this turn." },
    },
  };
}

function emptied(part: AgentPart): AgentPart {
  if (part.type === "text") {
    return { type: "text", text: "" };
  }
  if (part.type === "reasoning") {
    return { type: "reasoning", text: "" };
  }
  if (part.type === "tool_call") {
    return { type: "tool_call", id: part.id, name: part.name, input: "" };
  }
  return part;
}

function textOf(part: AgentPart): string {
  if (part.type === "text" || part.type === "reasoning") {
    return part.text;
  }
  return part.type === "tool_call" ? part.input : "";
}

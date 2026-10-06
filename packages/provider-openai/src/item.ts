import type { AgentPart } from "@goat/provider";
import { type JsonObject, stringOf, withoutKeys } from "./json.ts";
import { callItem, messageItem, reasoningItem } from "./wire.ts";

export type Draft =
  | { readonly type: "reasoning" | "text"; readonly text: string }
  | {
      readonly type: "tool_call";
      readonly id: string;
      readonly name: string;
      readonly input: string;
    };

export interface Finished {
  readonly parts: readonly (readonly [position: number, part: AgentPart])[];
  readonly refused: boolean;
}

export function partOf(draft: Draft): AgentPart {
  if (draft.type === "tool_call") {
    return { type: "tool_call", id: draft.id, name: draft.name, input: draft.input };
  }
  return draft.type === "reasoning"
    ? { type: "reasoning", text: draft.text }
    : { type: "text", text: draft.text };
}

export function finished(
  done: JsonObject,
  positions: readonly number[],
  textAt: (position: number) => string,
): Finished | undefined {
  const type = stringOf(done, "type");
  if (type === "reasoning") {
    return reasoning(done, positions, textAt);
  }
  if (type === "message") {
    return message(done);
  }
  if (type === "function_call") {
    return call(done);
  }
  return { parts: [[0, { type: "opaque", data: done }]], refused: false };
}

function reasoning(
  done: JsonObject,
  positions: readonly number[],
  textAt: (position: number) => string,
): Finished | undefined {
  const parsed = reasoningItem.safeParse(done);
  if (!parsed.success) {
    return undefined;
  }
  const summary = parsed.data.summary ?? [];
  const all = [...new Set([0, ...positions, ...summary.keys()])].toSorted(
    (left, right) => left - right,
  );
  const last = all.at(-1);
  const opaque = withoutKeys(done, ["type", "summary", "content", "status"]);
  return {
    parts: all.map((position) => [
      position,
      {
        type: "reasoning",
        text: summary[position]?.text ?? textAt(position),
        opaque: position === last ? opaque : { id: parsed.data.id },
      },
    ]),
    refused: false,
  };
}

function message(done: JsonObject): Finished | undefined {
  const parsed = messageItem.safeParse(done);
  if (!parsed.success) {
    return undefined;
  }
  const extras = withoutKeys(done, ["type", "role", "status", "content"]);
  let refused = false;
  const parts = parsed.data.content.flatMap((content: JsonObject, position): Finished["parts"] => {
    const refusal = content["type"] === "refusal";
    const text = stringOf(content, refusal ? "refusal" : "text");
    if (text === undefined) {
      return [];
    }
    refused ||= refusal;
    const annotations = content["annotations"];
    const opaque = {
      ...extras,
      ...(Array.isArray(annotations) && annotations.length > 0 ? { annotations } : {}),
      ...(refusal ? { refusal } : {}),
    };
    return [[position, { type: "text", text, opaque }]];
  });
  return { parts, refused };
}

function call(done: JsonObject): Finished | undefined {
  const parsed = callItem.safeParse(done);
  if (!parsed.success) {
    return undefined;
  }
  const { call_id: id, name, arguments: input } = parsed.data;
  const opaque = withoutKeys(done, ["type", "call_id", "name", "arguments", "status"]);
  const part: AgentPart = {
    type: "tool_call",
    id,
    name,
    input: input === "" ? "{}" : input,
    ...(Object.keys(opaque).length > 0 ? { opaque } : {}),
  };
  return { parts: [[0, part]], refused: false };
}

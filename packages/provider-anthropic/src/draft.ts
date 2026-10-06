import type { AgentPart, Json } from "../provider/index.ts";
import { type JsonObject, withoutKeys } from "./json.ts";

export type Draft =
  | {
      readonly type: "text";
      readonly text: string;
      readonly extra: JsonObject;
      readonly citations: readonly Json[];
    }
  | {
      readonly type: "reasoning";
      readonly text: string;
      readonly extra: JsonObject;
      readonly signature: string;
    }
  | {
      readonly type: "tool_call";
      readonly id: string;
      readonly name: string;
      readonly input: string;
      readonly extra: JsonObject;
    }
  | { readonly type: "opaque"; readonly data: JsonObject };

export type Delta =
  | { readonly type: "text_delta"; readonly text: string }
  | { readonly type: "thinking_delta"; readonly thinking: string }
  | { readonly type: "input_json_delta"; readonly partial_json: string }
  | { readonly type: "signature_delta"; readonly signature: string }
  | { readonly type: "citations_delta"; readonly citation: Json };

export function draftOf(block: JsonObject): Draft {
  const { type, text, thinking, signature, id, name, citations } = block;
  if (type === "text" && typeof text === "string") {
    return {
      type: "text",
      text,
      extra: withoutKeys(block, ["type", "text", "citations"]),
      citations: Array.isArray(citations) ? citations : [],
    };
  }
  if (type === "thinking" && typeof thinking === "string") {
    return {
      type: "reasoning",
      text: thinking,
      extra: withoutKeys(block, ["type", "thinking", "signature"]),
      signature: typeof signature === "string" ? signature : "",
    };
  }
  if (type === "tool_use" && typeof id === "string" && typeof name === "string") {
    const extra = withoutKeys(block, ["type", "id", "name", "input"]);
    return { type: "tool_call", id, name, input: "", extra };
  }
  return { type: "opaque", data: block };
}

export function apply(
  draft: Draft,
  change: Delta,
): { readonly draft: Draft; readonly text: string } {
  if (draft.type === "text" && change.type === "text_delta") {
    return { draft: { ...draft, text: draft.text + change.text }, text: change.text };
  }
  if (draft.type === "text" && change.type === "citations_delta") {
    return { draft: { ...draft, citations: [...draft.citations, change.citation] }, text: "" };
  }
  if (draft.type === "reasoning" && change.type === "thinking_delta") {
    return { draft: { ...draft, text: draft.text + change.thinking }, text: change.thinking };
  }
  if (draft.type === "reasoning" && change.type === "signature_delta") {
    return { draft: { ...draft, signature: change.signature }, text: "" };
  }
  if (draft.type === "tool_call" && change.type === "input_json_delta") {
    const input = draft.input + change.partial_json;
    return { draft: { ...draft, input }, text: change.partial_json };
  }
  return { draft, text: "" };
}

export function startPart(draft: Draft): AgentPart {
  if (draft.type === "text" || draft.type === "reasoning") {
    return { type: draft.type, text: draft.text };
  }
  if (draft.type === "tool_call") {
    return { type: "tool_call", id: draft.id, name: draft.name, input: draft.input };
  }
  return draft;
}

export function finalPart(draft: Draft): AgentPart {
  if (draft.type === "text") {
    const citations = draft.citations.length > 0 ? { citations: draft.citations } : {};
    return { type: "text", text: draft.text, ...opaque({ ...draft.extra, ...citations }) };
  }
  if (draft.type === "reasoning") {
    const signature = draft.signature === "" ? {} : { signature: draft.signature };
    return { type: "reasoning", text: draft.text, ...opaque({ ...draft.extra, ...signature }) };
  }
  if (draft.type === "tool_call") {
    const input = draft.input === "" ? "{}" : draft.input;
    return { type: "tool_call", id: draft.id, name: draft.name, input, ...opaque(draft.extra) };
  }
  return draft;
}

function opaque(object: JsonObject): { readonly opaque?: JsonObject } {
  return Object.keys(object).length > 0 ? { opaque: object } : {};
}

import type { ProviderError } from "./error.ts";
import type { Json } from "./json.ts";

export interface TextPart {
  readonly type: "text";
  readonly text: string;
  readonly opaque?: Json;
}

export interface ReasoningPart {
  readonly type: "reasoning";
  readonly text: string;
  readonly opaque?: Json;
}

export interface ToolCallPart {
  readonly type: "tool_call";
  readonly id: string;
  readonly name: string;
  readonly input: string;
  readonly opaque?: Json;
}

export interface ToolResultPart {
  readonly type: "tool_result";
  readonly callId: string;
  readonly parts: readonly (TextPart | MediaPart)[];
  readonly isError: boolean;
}

export interface OpaquePart {
  readonly type: "opaque";
  readonly data: Json;
}

export type MediaSource =
  | { readonly kind: "base64"; readonly data: string }
  | { readonly kind: "url"; readonly url: string };

export interface MediaPart {
  readonly type: "media";
  readonly mediaType: string;
  readonly source: MediaSource;
  readonly name?: string;
}

export type AgentPart = TextPart | ReasoningPart | ToolCallPart | OpaquePart;

export interface Producer {
  readonly provider: string;
  readonly model: string;
}

export type Stop =
  | { readonly reason: "done" | "length" | "overflow" | "refusal" | "aborted" }
  | { readonly reason: "error"; readonly error: ProviderError };

export interface Usage {
  readonly input: {
    readonly total: number;
    readonly cacheRead?: number;
    readonly cacheWrite?: number;
  };
  readonly output: { readonly total: number; readonly reasoning?: number };
  readonly cost?: number;
}

export interface SystemMessage {
  readonly role: "system";
  readonly parts: readonly TextPart[];
}

export interface UserMessage {
  readonly role: "user";
  readonly parts: readonly (TextPart | MediaPart)[];
}

export interface AgentMessage {
  readonly role: "agent";
  readonly producer: Producer;
  readonly parts: readonly AgentPart[];
  readonly stop: Stop;
  readonly usage?: Usage;
  readonly opaque?: Json;
}

export interface ToolMessage {
  readonly role: "tool";
  readonly parts: readonly ToolResultPart[];
}

export type Message = SystemMessage | UserMessage | AgentMessage | ToolMessage;

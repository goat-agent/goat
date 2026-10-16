import type { FunctionTool, Json, MediaPart, TextPart } from "@goat/provider";

export type JsonObject = { readonly [key: string]: Json };

export interface ToolContext {
  readonly signal: AbortSignal;
}

export interface ToolOutput {
  readonly parts: readonly (TextPart | MediaPart)[];
  readonly isError: boolean;
}

export interface Tool {
  readonly definition: FunctionTool;
  call(input: JsonObject, context: ToolContext): Promise<ToolOutput>;
}

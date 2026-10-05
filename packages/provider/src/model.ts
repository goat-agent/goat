import type { ProviderError } from "./error.ts";
import type { Event } from "./event.ts";
import type { JsonSchema } from "./json.ts";
import type { Message } from "./message.ts";
import type { Result } from "./result.ts";

export interface FunctionTool {
  readonly type: "function";
  readonly name: string;
  readonly description: string;
  readonly inputSchema: JsonSchema;
}

export type Tool = FunctionTool;

export interface Input {
  readonly messages: readonly Message[];
  readonly tools?: readonly Tool[];
  readonly effort?: string;
  readonly outputSchema?: JsonSchema;
  readonly maxOutputTokens?: number;
}

export interface ModelInfo {
  readonly id: string;
  readonly name: string;
  readonly contextWindow: number;
  readonly maxOutputTokens: number;
  readonly supports: {
    readonly media: readonly string[];
    readonly tools: boolean;
    readonly outputSchema: boolean;
  };
  readonly effort?: { readonly levels: readonly string[]; readonly default: string };
  readonly price?: {
    readonly input: number;
    readonly output: number;
    readonly cacheRead?: number;
    readonly cacheWrite?: number;
  };
}

export interface CallOptions {
  readonly signal?: AbortSignal;
}

export interface StreamOptions extends CallOptions {
  readonly conversationId?: string;
}

export interface Model {
  readonly id: string;
  readonly provider: string;
  info(options?: CallOptions): Promise<Result<ModelInfo, ProviderError>>;
  stream(input: Input, options?: StreamOptions): AsyncIterable<Event>;
}

export interface Provider {
  readonly id: string;
  model(id: string): Model;
  models(options?: CallOptions): Promise<Result<readonly ModelInfo[], ProviderError>>;
}

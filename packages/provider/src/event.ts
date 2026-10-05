import type { AgentMessage, AgentPart } from "./message.ts";

export type Event =
  | { readonly type: "part_start"; readonly index: number; readonly part: AgentPart }
  | { readonly type: "part_delta"; readonly index: number; readonly text: string }
  | { readonly type: "part_end"; readonly index: number; readonly part: AgentPart }
  | { readonly type: "end"; readonly message: AgentMessage };

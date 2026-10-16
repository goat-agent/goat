import type {
  AgentMessage,
  Event,
  Stop,
  ToolMessage,
  ToolResultPart,
  UserMessage,
} from "@goat/provider";

export type ScreenEvent =
  | { readonly type: "user"; readonly message: UserMessage }
  | Exclude<Event, { readonly type: "end" }>
  | { readonly type: "message"; readonly message: AgentMessage | ToolMessage }
  | { readonly type: "tool_result"; readonly result: ToolResultPart }
  | { readonly type: "end"; readonly stop: Stop };

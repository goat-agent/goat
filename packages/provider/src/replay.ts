import type { ProviderError } from "./error.ts";
import { isJsonObject } from "./json.ts";
import type {
  AgentMessage,
  AgentPart,
  MediaPart,
  Message,
  Producer,
  TextPart,
  ToolCallPart,
  ToolMessage,
  ToolResultPart,
} from "./message.ts";
import { err, ok, type Result } from "./result.ts";

export interface ReplayOptions {
  readonly foreign?: "beforeLastUser" | "all";
}

export function replay(
  messages: readonly Message[],
  target: Producer,
  options: ReplayOptions = {},
): Result<readonly Message[], ProviderError> {
  const kept = messages.filter((message) => isReplayable(message));
  const boundary = foreignBoundary(kept, options.foreign);
  const normalized = kept.map((message, index) => normalize(message, target, index < boundary));
  const ordered = orderToolResults(normalized.filter((message) => message.parts.length > 0));
  if (ordered.at(-1)?.role === "agent") {
    return err({ kind: "invalid", message: "A request must not end with an agent message." });
  }
  return ok(ordered);
}

function isReplayable(message: Message): boolean {
  return (
    message.role !== "agent" ||
    (message.stop.reason !== "error" && message.stop.reason !== "aborted")
  );
}

function foreignBoundary(messages: readonly Message[], foreign: ReplayOptions["foreign"]): number {
  if (foreign === "all") {
    return messages.length;
  }
  if (foreign === "beforeLastUser") {
    const lastUser = messages.findLastIndex((message) => message.role === "user");
    return lastUser === -1 ? messages.length : lastUser;
  }
  return 0;
}

function normalize(message: Message, target: Producer, forcedForeign: boolean): Message {
  if (message.role === "system") {
    return { role: "system", parts: message.parts.filter((part) => isFilledText(part)) };
  }
  if (message.role === "user") {
    return { role: "user", parts: withoutBlankText(message.parts) };
  }
  if (message.role === "tool") {
    return {
      role: "tool",
      parts: message.parts.map((result) => ({ ...result, parts: withoutBlankText(result.parts) })),
    };
  }
  return isForeign(message, target, forcedForeign)
    ? toForeign(message)
    : withoutBlankUnsignedText(message);
}

function isForeign(message: AgentMessage, target: Producer, forcedForeign: boolean): boolean {
  return (
    forcedForeign ||
    message.producer.provider !== target.provider ||
    message.producer.model !== target.model ||
    message.stop.reason === "length" ||
    message.parts.some((part) => isMalformedCall(part))
  );
}

function toForeign(message: AgentMessage): AgentMessage {
  return {
    role: "agent",
    producer: message.producer,
    parts: message.parts.flatMap((part) => foreignPart(part)),
    stop: message.stop,
  };
}

function foreignPart(part: AgentPart): readonly AgentPart[] {
  if (part.type === "text") {
    return isFilledText(part) ? [{ type: "text", text: part.text }] : [];
  }
  if (part.type === "tool_call") {
    return [{ type: "tool_call", id: part.id, name: part.name, input: part.input }];
  }
  return [];
}

function withoutBlankUnsignedText(message: AgentMessage): AgentMessage {
  return {
    ...message,
    parts: message.parts.filter(
      (part) => part.type !== "text" || part.opaque !== undefined || isFilledText(part),
    ),
  };
}

function withoutBlankText(
  parts: readonly (TextPart | MediaPart)[],
): readonly (TextPart | MediaPart)[] {
  return parts.filter((part) => part.type !== "text" || isFilledText(part));
}

function isFilledText(part: TextPart): boolean {
  return part.text.trim().length > 0;
}

function isMalformedCall(part: AgentPart): boolean {
  return part.type === "tool_call" && !isJsonObject(part.input);
}

function orderToolResults(messages: readonly Message[]): readonly Message[] {
  return segmentByAgent(messages).flatMap((segment) => orderSegment(segment));
}

function segmentByAgent(messages: readonly Message[]): readonly (readonly Message[])[] {
  const segments: Message[][] = [[]];
  for (const message of messages) {
    if (message.role === "agent") {
      segments.push([message]);
    } else {
      segments.at(-1)?.push(message);
    }
  }
  return segments;
}

function orderSegment(segment: readonly Message[]): readonly Message[] {
  const [head, ...rest] = segment;
  if (head?.role !== "agent") {
    return segment.filter((message) => message.role !== "tool");
  }
  const calls = head.parts.filter((part) => isToolCall(part)).map((call) => call.id);
  const results = firstResults(rest, calls);
  const answered = new Set(results.map((result) => result.callId));
  const missing = calls
    .filter((callId) => !answered.has(callId))
    .map((callId) => missingResult(callId));
  const others = rest.filter((message) => message.role !== "tool");
  return [
    head,
    ...results.map((result): ToolMessage => ({ role: "tool", parts: [result] })),
    ...missing,
    ...others,
  ];
}

function firstResults(
  messages: readonly Message[],
  calls: readonly string[],
): readonly ToolResultPart[] {
  const results = messages.flatMap((message) => (message.role === "tool" ? message.parts : []));
  return results.filter(
    (result, index) =>
      calls.includes(result.callId) &&
      results.findIndex((other) => other.callId === result.callId) === index,
  );
}

function isToolCall(part: AgentPart): part is ToolCallPart {
  return part.type === "tool_call";
}

function missingResult(callId: string): ToolMessage {
  return {
    role: "tool",
    parts: [
      {
        type: "tool_result",
        callId,
        parts: [{ type: "text", text: "No result was recorded for this call." }],
        isError: true,
      },
    ],
  };
}

import type { AgentMessage, Message, UserMessage } from "../message.ts";
import type { FunctionTool, Input } from "../model.ts";

export type FixtureName =
  | "text"
  | "reasoning"
  | "tool_call"
  | "parallel_tool_calls"
  | "malformed_tool_input"
  | "same_producer_replay"
  | "foreign_replay"
  | "auth_error"
  | "rate_limit"
  | "server_error"
  | "overflow"
  | "midstream_error";

export const weatherTool: FunctionTool = {
  type: "function",
  name: "get_weather",
  description: "Get the current weather for a city.",
  inputSchema: {
    type: "object",
    properties: { city: { type: "string" } },
    required: ["city"],
    additionalProperties: false,
  },
};

export const foreignMarkers = {
  signature: "conformance-foreign-signature",
  reasoning: "conformance-foreign-reasoning",
  textId: "conformance-foreign-text-id",
  text: "Hello from another model.",
} as const;

export const unsupportedEffort = "conformance-unsupported-effort";

export function scenarioInput(name: FixtureName, effort?: string): Input {
  if (name === "reasoning") {
    return withEffort(
      { messages: [user("What is 17 times 23? Think it through, then answer.")] },
      effort,
    );
  }
  if (name === "tool_call" || name === "malformed_tool_input") {
    return {
      messages: [user("What is the weather in Seoul? Use the tool.")],
      tools: [weatherTool],
    };
  }
  if (name === "parallel_tool_calls") {
    const request =
      "Get the weather in Seoul and in Tokyo. Call the tool for both cities in one turn.";
    return { messages: [user(request)], tools: [weatherTool] };
  }
  if (name === "same_producer_replay") {
    return withEffort(scenarioInput("tool_call", effort), effort);
  }
  if (name === "foreign_replay") {
    return { messages: foreignConversation() };
  }
  return {
    messages: [
      { role: "system", parts: [{ type: "text", text: "Answer in one short sentence." }] },
      user("Say hello."),
    ],
  };
}

export function secondTurn(first: Input, reply: AgentMessage): Input {
  const results: readonly Message[] = reply.parts.flatMap((part) =>
    part.type === "tool_call"
      ? [
          {
            role: "tool",
            parts: [
              {
                type: "tool_result",
                callId: part.id,
                parts: [{ type: "text", text: "Sunny, 21 degrees." }],
                isError: false,
              },
            ],
          },
        ]
      : [],
  );
  const next = results.length > 0 ? results : [user("Thanks. One more sentence, please.")];
  return { ...first, messages: [...first.messages, reply, ...next] };
}

function foreignConversation(): readonly Message[] {
  return [
    user("Say hello."),
    {
      role: "agent",
      producer: { provider: "conformance-foreign", model: "conformance-foreign-model" },
      parts: [
        {
          type: "reasoning",
          text: foreignMarkers.reasoning,
          opaque: { signature: foreignMarkers.signature },
        },
        { type: "text", text: foreignMarkers.text, opaque: { id: foreignMarkers.textId } },
      ],
      stop: { reason: "done" },
    },
    user("Say hello again."),
  ];
}

function withEffort(input: Input, effort: string | undefined): Input {
  return effort === undefined ? input : { ...input, effort };
}

function user(text: string): UserMessage {
  return { role: "user", parts: [{ type: "text", text }] };
}

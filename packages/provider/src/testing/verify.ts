import { expect } from "bun:test";
import { isJsonObject, type Json } from "../json.ts";
import type { AgentMessage, AgentPart, ToolCallPart } from "../message.ts";
import { weatherTool } from "./scenarios.ts";

export function expectText(message: AgentMessage): void {
  expect(message.stop.reason).toBe("done");
  expect(message.parts.some((part) => part.type === "text" && part.text.trim().length > 0)).toBe(
    true,
  );
  expect(message.parts.some((part) => part.type === "tool_call")).toBe(false);
}

export function expectReasoningFirst(message: AgentMessage): void {
  const reasoning = message.parts.findIndex((part) => part.type === "reasoning");
  const text = message.parts.findIndex((part) => part.type === "text");
  expect(reasoning).toBeGreaterThanOrEqual(0);
  expect(reasoning).toBeLessThan(text);
}

export function expectWeatherCall(message: AgentMessage): void {
  const calls = toolCalls(message);
  expect(calls.some((call) => call.name === weatherTool.name && isJsonObject(call.input))).toBe(
    true,
  );
  expect(message.stop.reason).toBe("done");
}

export function expectParallelCalls(message: AgentMessage): void {
  const ids = toolCalls(message).map((call) => call.id);
  expect(ids.length).toBeGreaterThanOrEqual(2);
  expect(new Set(ids).size).toBe(ids.length);
}

export function expectMalformedCall(message: AgentMessage): void {
  expect(toolCalls(message).some((call) => !isJsonObject(call.input))).toBe(true);
  expect(message.stop.reason).not.toBe("error");
}

export function opaqueStrings(message: AgentMessage): readonly string[] {
  const values: readonly Json[] = [
    ...(message.opaque === undefined ? [] : [message.opaque]),
    ...message.parts.flatMap((part) => opaqueOf(part)),
  ];
  return values.flatMap((value) => stringLeaves(value));
}

function toolCalls(message: AgentMessage): readonly ToolCallPart[] {
  return message.parts.filter((part): part is ToolCallPart => part.type === "tool_call");
}

function opaqueOf(part: AgentPart): readonly Json[] {
  if (part.type === "opaque") {
    return [part.data];
  }
  return part.opaque === undefined ? [] : [part.opaque];
}

function stringLeaves(value: Json): readonly string[] {
  if (typeof value === "string") {
    return [value];
  }
  if (value === null || typeof value !== "object") {
    return [];
  }
  return (Array.isArray(value) ? value : Object.values(value)).flatMap((child: Json) =>
    stringLeaves(child),
  );
}

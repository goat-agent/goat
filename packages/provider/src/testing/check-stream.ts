import { isDeepStrictEqual } from "node:util";
import type { Event } from "../event.ts";
import type { AgentMessage, AgentPart } from "../message.ts";
import { err, ok, type Result } from "../result.ts";

interface StreamState {
  readonly started: readonly AgentPart[];
  readonly ended: ReadonlyMap<number, AgentPart>;
  readonly message: AgentMessage | undefined;
  readonly violations: readonly string[];
}

export function checkStream(events: readonly Event[]): Result<AgentMessage, readonly string[]> {
  let state: StreamState = { started: [], ended: new Map(), message: undefined, violations: [] };
  for (const event of events) {
    state = step(state, event);
  }
  if (state.message === undefined) {
    return err([...state.violations, "The stream has no end event."]);
  }
  return state.violations.length > 0 ? err(state.violations) : ok(state.message);
}

function step(state: StreamState, event: Event): StreamState {
  if (state.message !== undefined) {
    return violate(state, `A ${event.type} event arrived after end.`);
  }
  if (event.type === "part_start") {
    return startPart(state, event.index, event.part);
  }
  if (event.type === "part_delta") {
    return deltaPart(state, event.index);
  }
  if (event.type === "part_end") {
    return endPart(state, event.index, event.part);
  }
  return finish(state, event.message);
}

function startPart(state: StreamState, index: number, part: AgentPart): StreamState {
  if (index !== state.started.length) {
    return violate(
      state,
      `part_start used index ${String(index)}, expected ${String(state.started.length)}.`,
    );
  }
  return { ...state, started: [...state.started, part] };
}

function deltaPart(state: StreamState, index: number): StreamState {
  const started = state.started[index];
  if (started === undefined) {
    return violate(state, `part_delta for part ${String(index)} arrived before its part_start.`);
  }
  if (state.ended.has(index)) {
    return violate(state, `part_delta for part ${String(index)} arrived after its part_end.`);
  }
  if (started.type === "opaque") {
    return violate(state, `part_delta arrived for opaque part ${String(index)}.`);
  }
  return state;
}

function endPart(state: StreamState, index: number, part: AgentPart): StreamState {
  const started = state.started[index];
  if (started === undefined) {
    return violate(state, `part_end for part ${String(index)} arrived before its part_start.`);
  }
  if (state.ended.has(index)) {
    return violate(state, `part_end for part ${String(index)} arrived twice.`);
  }
  if (started.type !== part.type) {
    return violate(
      state,
      `part ${String(index)} started as ${started.type} and ended as ${part.type}.`,
    );
  }
  return { ...state, ended: new Map([...state.ended, [index, part]]) };
}

function finish(state: StreamState, message: AgentMessage): StreamState {
  const unclosed = state.started.flatMap((_, index) => (state.ended.has(index) ? [] : [index]));
  const mismatched = state.started.flatMap((_, index) =>
    isDeepStrictEqual(state.ended.get(index), message.parts[index]) ? [] : [index],
  );
  const violations = [
    ...unclosed.map((index) => `part ${String(index)} was never ended.`),
    ...(message.parts.length === state.started.length
      ? []
      : [
          `end has ${String(message.parts.length)} parts but ${String(state.started.length)} were started.`,
        ]),
    ...mismatched.map((index) => `end part ${String(index)} differs from its part_end.`),
  ];
  return { ...state, message, violations: [...state.violations, ...violations] };
}

function violate(state: StreamState, violation: string): StreamState {
  return { ...state, violations: [...state.violations, violation] };
}

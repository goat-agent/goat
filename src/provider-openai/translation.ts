import type { AgentPart, Event, Producer, Stop } from "../provider/index.ts";
import { z } from "zod";
import type { Profile } from "./catalog.ts";
import { eventFailure, type Failure, malformed } from "./error.ts";
import { outcomeOf } from "./outcome.ts";
import { type Draft, finished, partOf } from "./item.ts";
import { stringOf } from "./json.ts";
import { usageOf } from "./usage.ts";
import {
  callItem,
  contentDelta,
  contentPart,
  errorEvent,
  item,
  outputDelta,
  summaryDelta,
  summaryPart,
  terminal,
  type WireResponse,
} from "./wire.ts";

interface Opened {
  readonly index: number;
  readonly events: readonly Event[];
}

const eventType = z.object({ type: z.string() });

export class Translation {
  readonly #producer: Producer;
  readonly #profile: Profile | undefined;
  readonly #positions = new Map<number, Map<number, number>>();
  readonly #drafts = new Map<number, Draft>();
  readonly #parts = new Map<number, AgentPart>();
  #refused = false;
  #failure: Failure | undefined;
  #finished = false;

  constructor(producer: Producer, profile: Profile | undefined) {
    this.#producer = producer;
    this.#profile = profile;
  }

  get finished(): boolean {
    return this.#finished;
  }

  get started(): boolean {
    return this.#parts.size > 0;
  }

  get failure(): Failure | undefined {
    return this.#failure;
  }

  push(data: unknown): readonly Event[] {
    const parsed = eventType.safeParse(data);
    if (this.#finished || !parsed.success) {
      return [];
    }
    const { type } = parsed.data;
    switch (type) {
      case "response.output_item.added": {
        return this.#itemAdded(data);
      }
      case "response.reasoning_summary_part.added":
      case "response.reasoning_summary_text.delta": {
        return this.#summary(type, data);
      }
      case "response.content_part.added":
      case "response.output_text.delta":
      case "response.refusal.delta": {
        return this.#content(type, data);
      }
      case "response.function_call_arguments.delta": {
        return this.#arguments(data);
      }
      case "response.output_item.done": {
        return this.#itemDone(data);
      }
      case "response.completed":
      case "response.incomplete":
      case "response.failed": {
        return this.#terminal(type, data);
      }
      case "error": {
        const failure = errorEvent.safeParse(data);
        return failure.success
          ? this.#fail(eventFailure(failure.data.error ?? failure.data))
          : this.#malformed(type);
      }
      default: {
        return [];
      }
    }
  }

  close(stop: Stop, response?: WireResponse): readonly Event[] {
    if (this.#finished) {
      return [];
    }
    const events = [...this.#drafts.keys()].map((index) => this.#end(index));
    this.#finished = true;
    const usage = usageOf(response?.usage, response?.service_tier ?? undefined, this.#profile);
    events.push({
      type: "end",
      message: {
        role: "agent",
        producer: this.#producer,
        parts: [...this.#parts.values()],
        stop,
        ...(usage === undefined ? {} : { usage }),
      },
    });
    return events;
  }

  #itemAdded(data: unknown): readonly Event[] {
    const parsed = item.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("response.output_item.added");
    }
    const { output_index: output, item: added } = parsed.data;
    const call = callItem.safeParse(added);
    if (added["type"] !== "function_call" || !call.success) {
      return [];
    }
    const { call_id: id, name } = call.data;
    return [this.#start(output, 0, { type: "tool_call", id, name, input: "" })];
  }

  #summary(type: string, data: unknown): readonly Event[] {
    const parsed = (type.endsWith("delta") ? summaryDelta : summaryPart).safeParse(data);
    if (!parsed.success) {
      return this.#malformed(type);
    }
    const { output_index: output, summary_index: position } = parsed.data;
    const opened = this.#open(output, position, "reasoning");
    const delta = "delta" in parsed.data ? parsed.data.delta : "";
    return [...opened.events, ...this.#append(opened.index, delta)];
  }

  #content(type: string, data: unknown): readonly Event[] {
    const parsed = (type.endsWith("delta") ? contentDelta : contentPart).safeParse(data);
    if (!parsed.success) {
      return this.#malformed(type);
    }
    const opened = this.#open(parsed.data.output_index, parsed.data.content_index, "text");
    const delta = "delta" in parsed.data ? parsed.data.delta : "";
    return [...opened.events, ...this.#append(opened.index, delta)];
  }

  #arguments(data: unknown): readonly Event[] {
    const parsed = outputDelta.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("response.function_call_arguments.delta");
    }
    const index = this.#positionsOf(parsed.data.output_index).get(0);
    return index === undefined ? [] : this.#append(index, parsed.data.delta);
  }

  #itemDone(data: unknown): readonly Event[] {
    const parsed = item.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("response.output_item.done");
    }
    const { output_index: output, item: done } = parsed.data;
    const positions = this.#positionsOf(output);
    const result = finished(done, [...positions.keys()], (position) =>
      this.#textOf(positions.get(position)),
    );
    if (result === undefined) {
      return this.#malformed(`${stringOf(done, "type") ?? "unknown"} item`);
    }
    this.#refused ||= result.refused;
    return result.parts.flatMap(([position, part]) => this.#finish(output, position, part));
  }

  #terminal(type: string, data: unknown): readonly Event[] {
    const parsed = terminal.safeParse(data);
    if (!parsed.success) {
      return this.#malformed(type);
    }
    const { response } = parsed.data;
    const outcome = outcomeOf(type, response, this.#refused);
    return "failure" in outcome
      ? this.#fail(outcome.failure, response)
      : this.close(outcome.stop, response);
  }

  #fail(failure: Failure, response?: WireResponse): readonly Event[] {
    this.#failure = failure;
    return this.close({ reason: "error", error: failure.error }, response);
  }

  #open(output: number, position: number, type: "reasoning" | "text"): Opened {
    const known = this.#positionsOf(output).get(position);
    if (known !== undefined) {
      return { index: known, events: [] };
    }
    const started = this.#start(output, position, { type, text: "" });
    return { index: this.#positionsOf(output).get(position) ?? -1, events: [started] };
  }

  #positionsOf(output: number): Map<number, number> {
    const known = this.#positions.get(output);
    if (known !== undefined) {
      return known;
    }
    const created = new Map<number, number>();
    this.#positions.set(output, created);
    return created;
  }

  #claim(output: number, position: number): number {
    const index = this.#parts.size;
    this.#positionsOf(output).set(position, index);
    return index;
  }

  #start(output: number, position: number, draft: Draft): Event {
    const index = this.#claim(output, position);
    const part = partOf(draft);
    this.#drafts.set(index, draft);
    this.#parts.set(index, part);
    return { type: "part_start", index, part };
  }

  #append(index: number, text: string): readonly Event[] {
    const draft = this.#drafts.get(index);
    if (draft === undefined || text === "") {
      return [];
    }
    this.#drafts.set(
      index,
      draft.type === "tool_call"
        ? { ...draft, input: draft.input + text }
        : { ...draft, text: draft.text + text },
    );
    return [{ type: "part_delta", index, text }];
  }

  #finish(output: number, position: number, part: AgentPart): readonly Event[] {
    const known = this.#positionsOf(output).get(position);
    if (known === undefined) {
      const index = this.#claim(output, position);
      this.#parts.set(index, part);
      return [
        { type: "part_start", index, part },
        { type: "part_end", index, part },
      ];
    }
    return this.#drafts.has(known) ? [this.#end(known, part)] : [];
  }

  #end(index: number, final?: AgentPart): Event {
    const draft = this.#drafts.get(index);
    this.#drafts.delete(index);
    const part = final ?? (draft === undefined ? undefined : partOf(draft));
    const settled = part ?? this.#parts.get(index) ?? { type: "text", text: "" };
    this.#parts.set(index, settled);
    return { type: "part_end", index, part: settled };
  }

  #textOf(index: number | undefined): string {
    const draft = index === undefined ? undefined : this.#drafts.get(index);
    if (draft !== undefined) {
      return draft.type === "tool_call" ? draft.input : draft.text;
    }
    const part = index === undefined ? undefined : this.#parts.get(index);
    return part?.type === "reasoning" || part?.type === "text" ? part.text : "";
  }

  #malformed(event: string): readonly Event[] {
    return this.close({
      reason: "error",
      error: malformed(`OpenAI sent a ${event} event that does not match its schema.`),
    });
  }
}

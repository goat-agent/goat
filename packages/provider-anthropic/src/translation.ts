import type { AgentPart, Event, Producer, Stop } from "@goat/provider";
import type { ServerSentEvent } from "@goat/sse";
import type { Price } from "./catalog.ts";
import { apply, type Draft, draftOf, finalPart, startPart } from "./draft.ts";
import { eventError, malformed } from "./error.ts";
import { addCounts, type Counts, noCounts, usageOf } from "./usage.ts";
import {
  blockDelta,
  blockStart,
  blockStop,
  delta,
  errorEvent,
  messageDelta,
  messageStart,
  type WireUsage,
} from "./wire.ts";

export class Translation {
  readonly #producer: Producer;
  readonly #price: Price | undefined;
  readonly #open = new Map<number, Draft>();
  readonly #parts = new Map<number, AgentPart>();
  #counts: Counts = noCounts;
  #stopReason: string | undefined;
  #finished = false;

  constructor(producer: Producer, price: Price | undefined) {
    this.#producer = producer;
    this.#price = price;
  }

  get finished(): boolean {
    return this.#finished;
  }

  push(event: ServerSentEvent): readonly Event[] {
    if (this.#finished) {
      return [];
    }
    const data = parse(event.data);
    switch (event.type) {
      case "message_start": {
        return this.#messageStart(data);
      }
      case "content_block_start": {
        return this.#blockStart(data);
      }
      case "content_block_delta": {
        return this.#blockDelta(data);
      }
      case "content_block_stop": {
        return this.#blockStop(data);
      }
      case "message_delta": {
        return this.#messageDelta(data);
      }
      case "message_stop": {
        return this.close(stopOf(this.#stopReason));
      }
      case "error": {
        return this.#error(data);
      }
      default: {
        return [];
      }
    }
  }

  close(stop: Stop): readonly Event[] {
    if (this.#finished) {
      return [];
    }
    const events: Event[] = [];
    for (const [index, draft] of this.#open) {
      events.push(this.#end(index, draft));
    }
    this.#finished = true;
    const usage = usageOf(this.#counts, this.#price);
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

  #messageStart(data: unknown): readonly Event[] {
    const parsed = messageStart.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("message_start");
    }
    this.#count(parsed.data.message.usage);
    return [];
  }

  #blockStart(data: unknown): readonly Event[] {
    const parsed = blockStart.safeParse(data);
    if (!parsed.success || this.#parts.has(parsed.data.index)) {
      return this.#malformed("content_block_start");
    }
    const { index } = parsed.data;
    const draft = draftOf(parsed.data.content_block);
    const part = startPart(draft);
    this.#open.set(index, draft);
    this.#parts.set(index, part);
    return [{ type: "part_start", index, part }];
  }

  #blockDelta(data: unknown): readonly Event[] {
    const parsed = blockDelta.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("content_block_delta");
    }
    const { index } = parsed.data;
    const draft = this.#open.get(index);
    const change = delta.safeParse(parsed.data.delta);
    if (draft === undefined || !change.success) {
      return [];
    }
    const applied = apply(draft, change.data);
    this.#open.set(index, applied.draft);
    return applied.text === "" ? [] : [{ type: "part_delta", index, text: applied.text }];
  }

  #blockStop(data: unknown): readonly Event[] {
    const parsed = blockStop.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("content_block_stop");
    }
    const draft = this.#open.get(parsed.data.index);
    return draft === undefined ? [] : [this.#end(parsed.data.index, draft)];
  }

  #messageDelta(data: unknown): readonly Event[] {
    const parsed = messageDelta.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("message_delta");
    }
    this.#stopReason = parsed.data.delta.stop_reason ?? this.#stopReason;
    this.#count(parsed.data.usage);
    return [];
  }

  #error(data: unknown): readonly Event[] {
    const parsed = errorEvent.safeParse(data);
    if (!parsed.success) {
      return this.#malformed("error");
    }
    const { type, message } = parsed.data.error;
    return this.close({ reason: "error", error: eventError(type, message) });
  }

  #end(index: number, draft: Draft): Event {
    const part = finalPart(draft);
    this.#open.delete(index);
    this.#parts.set(index, part);
    return { type: "part_end", index, part };
  }

  #count(usage: WireUsage | null | undefined): void {
    if (usage !== undefined && usage !== null) {
      this.#counts = addCounts(this.#counts, usage);
    }
  }

  #malformed(event: string): readonly Event[] {
    return this.close({
      reason: "error",
      error: malformed(`Anthropic sent a ${event} event that does not match its schema.`),
    });
  }
}

function parse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function stopOf(reason: string | undefined): Stop {
  if (reason === undefined) {
    const error = malformed("Anthropic ended the message without a stop reason.");
    return { reason: "error", error };
  }
  if (reason === "max_tokens") {
    return { reason: "length" };
  }
  if (reason === "model_context_window_exceeded") {
    return { reason: "overflow" };
  }
  return { reason: reason === "refusal" ? "refusal" : "done" };
}

import {
  type Event,
  type Input,
  type Message,
  type ModelInfo,
  type Producer,
  type ProviderError,
  type ReplayOptions,
  type Result,
  replay,
  type Stop,
  type StreamOptions,
} from "@goat/provider";
import { decode } from "@goat/sse";
import type { Profile } from "./catalog.ts";
import type { Client } from "./client.ts";
import { isOpaqueRejection, networkError, responseError } from "./error.ts";
import { request } from "./request.ts";
import { Translation } from "./translation.ts";

export interface Turn {
  readonly client: Client;
  readonly producer: Producer;
  readonly profile: (signal: AbortSignal) => Promise<Result<Profile, ProviderError>>;
}

interface Context {
  readonly client: Client;
  readonly producer: Producer;
  readonly profile: Profile;
  readonly input: Input;
  readonly messages: readonly Message[];
  readonly signal: AbortSignal;
  readonly cancelled: () => boolean;
}

interface Rejected {
  readonly body: string;
  readonly error: ProviderError;
}

const ladder: readonly ReplayOptions[] = [{}, { foreign: "beforeLastUser" }, { foreign: "all" }];
const toolName = /^[\w-]{1,128}$/u;

export async function* streamTurn(
  turn: Turn,
  input: Input,
  options: StreamOptions,
): AsyncGenerator<Event> {
  const controller = new AbortController();
  const signal =
    options.signal === undefined
      ? controller.signal
      : AbortSignal.any([options.signal, controller.signal]);
  const cancelled = (): boolean => options.signal?.aborted === true;
  try {
    const profile = await turn.profile(signal);
    if (!profile.ok) {
      yield end(turn.producer, stopFor(profile.error, cancelled));
      return;
    }
    const problem = preflight(input, profile.value.info);
    if (problem !== undefined) {
      yield end(turn.producer, { reason: "error", error: problem });
      return;
    }
    const messages = input.messages.filter((message) => !isOwnRefusal(message, turn.producer));
    const context = { ...turn, profile: profile.value, input, messages, signal, cancelled };
    yield* attempt(context, 0);
  } finally {
    controller.abort();
  }
}

async function* attempt(
  context: Context,
  level: number,
  previous?: Rejected,
): AsyncGenerator<Event> {
  const replayed = replay(context.messages, context.producer, ladder[level] ?? {});
  if (!replayed.ok) {
    yield end(context.producer, { reason: "error", error: replayed.error });
    return;
  }
  const { body: wire, betas } = request(
    context.producer.model,
    context.input,
    replayed.value,
    context.profile,
  );
  const body = JSON.stringify(wire);
  const last = level === ladder.length - 1;
  if (previous !== undefined && body === previous.body) {
    if (last) {
      yield end(context.producer, { reason: "error", error: previous.error });
    } else {
      yield* attempt(context, level + 1, previous);
    }
    return;
  }
  const response = await context.client.send("messages", {
    method: "POST",
    body,
    betas,
    signal: context.signal,
  });
  if (!response.ok) {
    yield end(context.producer, stopFor(response.error, context.cancelled));
    return;
  }
  if (!response.value.ok) {
    const error = await failureOf(response.value);
    if (!last && isOpaqueRejection(response.value.status, error)) {
      yield* attempt(context, level + 1, { body, error });
    } else {
      yield end(context.producer, stopFor(error, context.cancelled));
    }
    return;
  }
  yield* read(context, response.value.body);
}

async function* read(
  context: Context,
  body: ReadableStream<Uint8Array> | null,
): AsyncGenerator<Event> {
  const translation = new Translation(context.producer, context.profile.price);
  try {
    if (body !== null) {
      for await (const event of decode(body)) {
        if (context.cancelled()) {
          yield* translation.close({ reason: "aborted" });
          return;
        }
        yield* translation.push(event);
        if (translation.finished) {
          return;
        }
      }
    }
    const error = networkError("Anthropic closed the stream before the message finished.");
    yield* translation.close({ reason: "error", error });
  } catch (error) {
    yield* translation.close(stopFor(networkError(error), context.cancelled));
  }
}

async function failureOf(response: Response): Promise<ProviderError> {
  try {
    return responseError(response.status, response.headers, await response.text());
  } catch (error) {
    return networkError(error);
  }
}

function preflight(input: Input, info: ModelInfo): ProviderError | undefined {
  if (input.effort !== undefined && !(info.effort?.levels ?? []).includes(input.effort)) {
    return unsupported(`${info.id} does not support effort "${input.effort}".`);
  }
  if (input.outputSchema !== undefined && !info.supports.outputSchema) {
    return unsupported(`${info.id} does not support output schemas.`);
  }
  if ((input.tools?.length ?? 0) > 0 && !info.supports.tools) {
    return unsupported(`${info.id} does not support tools.`);
  }
  const media = mediaTypes(input.messages).find((type) => !info.supports.media.includes(type));
  if (media !== undefined) {
    return unsupported(`${info.id} does not accept ${media}.`);
  }
  const tool = input.tools?.find((candidate) => !toolName.test(candidate.name));
  return tool === undefined
    ? undefined
    : { kind: "invalid", message: `Tool name "${tool.name}" must match ${toolName.source}.` };
}

function unsupported(message: string): ProviderError {
  return { kind: "unsupported", message };
}

function mediaTypes(messages: readonly Message[]): readonly string[] {
  return messages.flatMap((message) => {
    if (message.role === "user") {
      return message.parts.flatMap((part) => (part.type === "media" ? [part.mediaType] : []));
    }
    if (message.role === "tool") {
      return message.parts.flatMap((result) =>
        result.parts.flatMap((part) => (part.type === "media" ? [part.mediaType] : [])),
      );
    }
    return [];
  });
}

function isOwnRefusal(message: Message, producer: Producer): boolean {
  return (
    message.role === "agent" &&
    message.producer.provider === producer.provider &&
    message.stop.reason === "refusal"
  );
}

function stopFor(error: ProviderError, cancelled: () => boolean): Stop {
  return cancelled() ? { reason: "aborted" } : { reason: "error", error };
}

function end(producer: Producer, stop: Stop): Event {
  return { type: "end", message: { role: "agent", producer, parts: [], stop } };
}

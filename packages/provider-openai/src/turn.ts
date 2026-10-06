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
import {
  type Failure,
  isOpaqueRejection,
  isSummaryRejection,
  networkError,
  responseFailure,
} from "./error.ts";
import { request } from "./request.ts";
import { Translation } from "./translation.ts";

export interface Route {
  readonly namespaced: boolean;
  readonly maxOutputTokens: boolean;
  readonly serviceTier?: string;
}

export class Settings {
  #summaries = true;

  get summaries(): boolean {
    return this.#summaries;
  }

  withoutSummaries(): void {
    this.#summaries = false;
  }
}

export interface Turn {
  readonly client: Client;
  readonly producer: Producer;
  readonly route: Route;
  readonly settings: Settings;
  readonly profile: (signal: AbortSignal) => Promise<Result<Profile | undefined, ProviderError>>;
}

interface Context extends Turn {
  readonly known: Profile | undefined;
  readonly input: Input;
  readonly messages: readonly Message[];
  readonly options: StreamOptions;
  readonly signal: AbortSignal;
  readonly cancelled: () => boolean;
}

interface Rejected {
  readonly body: string;
  readonly error: ProviderError;
}

const ladder: readonly ReplayOptions[] = [{}, { foreign: "beforeLastUser" }, { foreign: "all" }];
const toolName = /^[\w-]{1,64}$/u;

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
    const problem = preflight(input, profile.value?.info);
    if (problem !== undefined) {
      yield end(turn.producer, { reason: "error", error: problem });
      return;
    }
    const messages = input.messages.filter((message) => !isOwnRefusal(message, turn.producer));
    const known = profile.value;
    yield* attempt({ ...turn, known, input, messages, options, signal, cancelled }, 0);
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
  const body = bodyOf(context, replayed.value);
  const last = level === ladder.length - 1;
  if (previous !== undefined && body === previous.body) {
    if (last) {
      yield end(context.producer, { reason: "error", error: previous.error });
    } else {
      yield* attempt(context, level + 1, previous);
    }
    return;
  }
  const retriable = (failure: Failure): boolean =>
    (context.settings.summaries && isSummaryRejection(failure)) ||
    (!last && isOpaqueRejection(failure));
  const response = await context.client.post("responses", body, context.signal);
  if (!response.ok) {
    yield end(context.producer, stopFor(response.error, context.cancelled));
    return;
  }
  const rejected = response.value.ok
    ? yield* read(context, response.value.body, retriable)
    : await failureOf(response.value);
  if (rejected === undefined) {
    return;
  }
  if (context.settings.summaries && isSummaryRejection(rejected)) {
    context.settings.withoutSummaries();
    yield* attempt(context, level);
  } else if (!last && isOpaqueRejection(rejected)) {
    yield* attempt(context, level + 1, { body, error: rejected.error });
  } else {
    yield end(context.producer, stopFor(rejected.error, context.cancelled));
  }
}

function bodyOf(context: Context, messages: readonly Message[]): string {
  const { conversationId } = context.options;
  return JSON.stringify(
    request(context.producer.model, context.input, messages, {
      ...context.route,
      summaries: context.settings.summaries,
      ...(conversationId === undefined ? {} : { conversationId }),
    }),
  );
}

async function* read(
  context: Context,
  body: ReadableStream<Uint8Array> | null,
  retriable: (failure: Failure) => boolean,
): AsyncGenerator<Event, Failure | undefined> {
  const translation = new Translation(context.producer, context.known);
  try {
    if (body !== null) {
      for await (const event of decode(body)) {
        if (context.cancelled()) {
          yield* translation.close({ reason: "aborted" });
          return undefined;
        }
        const events = translation.push(parse(event.data));
        const { failure } = translation;
        if (!translation.started && failure !== undefined && retriable(failure)) {
          return failure;
        }
        yield* events;
        if (translation.finished) {
          return undefined;
        }
      }
    }
    const error = networkError("OpenAI closed the stream before the response finished.");
    yield* translation.close({ reason: "error", error });
  } catch (error) {
    yield* translation.close(stopFor(networkError(error), context.cancelled));
  }
  return undefined;
}

async function failureOf(response: Response): Promise<Failure> {
  try {
    return responseFailure(response.status, response.headers, await response.text());
  } catch (error) {
    return { error: networkError(error) };
  }
}

function parse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function preflight(input: Input, info: ModelInfo | undefined): ProviderError | undefined {
  const tool = input.tools?.find((candidate) => !toolName.test(candidate.name));
  if (tool !== undefined) {
    return { kind: "invalid", message: `Tool name "${tool.name}" must match ${toolName.source}.` };
  }
  if (info === undefined) {
    return undefined;
  }
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
  return media === undefined ? undefined : unsupported(`${info.id} does not accept ${media}.`);
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

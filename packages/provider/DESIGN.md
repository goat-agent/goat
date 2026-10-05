# @goat/provider

The abstraction every goat product uses to talk to language models. Implementations live in separate packages (`@goat/provider-anthropic`, `@goat/provider-openai`, ...). They depend on this package; this package never knows them. Consumers depend only on this package.

The types are in `src/`. This document records why they look the way they do, and the contract every implementation must keep.

## Principles

1. Only intent and facts. Mechanisms such as caching, wire formats and authentication stay in implementations.
2. Only concepts that exist in at least two providers, or that are essential to a model continuing a conversation.
3. What varies per model is data in `ModelInfo`, not types.
4. Data a provider produces round-trips to that provider without loss, through `opaque`.
5. Evolution only adds. Nothing is renamed or removed, so persisted conversations stay valid.
6. No derivable fields and no hidden invariants. Types enforce the rules.
7. Failure is data. `stream` never throws and ends with exactly one `end` event.
8. Units are fixed once: quantities in tokens, money in USD, durations in milliseconds, prices per million tokens.

## Shape

- `Provider` creates models and lists them. Each implementation has its own constructors (for example `anthropic.apiKey(...)`, `anthropic.oauth(...)`), and all of them return a `Provider`.
- `Model` is what consumers use. `info()` is asynchronous because some models can only be described by asking a server. `stream()` returns an `AsyncIterable<Event>`.
- `Input` is pure data: what to generate. `StreamOptions` is how to run one call: cancellation and the conversation id that some providers use to route requests to a warm cache.
- Messages have four roles: `system`, `user`, `agent`, `tool`. Content is an ordered list of parts because providers require the original order of reasoning, text and tool calls.
- Tool restrictions such as plan mode belong to the agent loop, not to `Input`. Keeping `tools` and `effort` stable during a session preserves provider caches.

## Contract

1. A stop reason means the model produced output and stopped for that reason. `{ reason: "error" }` means the request failed. Context overflow and refusal therefore appear as a stop reason when generation halted, and as an error kind when the request was rejected.
2. The agent loop runs tools when an agent message contains tool calls and did not end with `error` or `aborted`. A call whose input is not a JSON object gets an error result instead of running.
3. `producer.provider` names the scope in which the producer's opaque data is valid, so backends with incompatible opaque data are different providers. `producer.model` is the id passed to `Provider.model()`. An implementation that cannot use opaque data treats the message as foreign and never fails because of it.
4. Several parts may be open at once; `index` tells them apart. Deltas append to `text` for text and reasoning parts and to `input` for tool calls. `part_end` carries the authoritative final part. `end.message` contains every started part in its final state.
5. A capability the model lacks is rejected as `unsupported` without a network call when `ModelInfo` is known. Otherwise the provider's rejection is mapped to an error kind.
6. `effort.levels` run from weakest to strongest. `maxOutputTokens` includes reasoning. `supports.media` lists exact MIME types accepted anywhere in the input. `price` is the base list price; `usage.cost` is authoritative. `usage` is the billed sum for the agent message.
7. Within one version the unions are closed. Loaders of persisted data from a newer version keep unknown variants verbatim and leave them out of requests.
8. A request must not end with an agent message.

## Replay

`replay(messages, target, options)` prepares stored messages for a request. It is pure and every implementation calls it before encoding.

1. Agent messages that ended with `error` or `aborted` are dropped.
2. An agent message is foreign when another producer made it, when it stopped at the length limit, when it contains a malformed tool call, or when `options.foreign` says so. Foreign messages keep plain text and tool calls only.
3. Blank text is dropped everywhere, except same-producer parts that carry opaque data.
4. Results follow their calls directly. Other messages that sit between a call and its results move after the results. Calls without results get a synthesized error result. Results without calls are dropped.
5. Messages left without parts are dropped.
6. A request that would end with an agent message is rejected as `invalid`.

## Implementation duties

- When the provider rejects a request because of replayed opaque data, and no event has been emitted yet, re-send once with `foreign: "beforeLastUser"` and then once with `foreign: "all"`.
- Handle provider quirks internally: continuing paused turns, mapping stop reasons, normalizing tool call ids without rewriting stored ids, sending `{}` for malformed calls where the wire format needs an object, omitting the provider's own refused turns, and placing non-leading system messages.
- Pass the shared conformance suite.

## Accepted costs

- A stored opaque value the provider no longer accepts costs one rejected request on each later turn.
- Switching to another producer loses earlier reasoning.

## Extension points

Each of these is an addition, not a change: media output parts, provider-run tools and their results, citations on text, binary deltas for audio, new tool kinds, provider file references as a media source, new usage and price fields, a context occupancy field, and new model kinds such as embeddings or realtime voice as separate `Provider` methods.

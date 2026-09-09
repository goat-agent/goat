# AGENTS.md — goat-provider

The `Provider` trait and the vocabulary every provider crate speaks. `goat-providers` assembles them;
read that crate's `AGENTS.md` first, since most providers need no crate at all.

## The trait

Required: `id`, `capabilities`, `stream(Request) -> ChunkStream`, `discover(mpsc::Sender<Model>)`.

`metadata`, `list_models` and `model_list_source` have defaults. Returning a non-empty `list_models`
flips `ModelListSource` from `Discover` to `Catalog`, so a catalog provider need not also override
the source.

## `StreamChunk` and `StreamError` are not `#[non_exhaustive]`

They are the provider vocabulary. Adding a variant moves every provider carrying it at once, and the
compiler is the checklist.

`StreamChunk` covers `TextDelta`, `ThinkingDelta`, `ThinkingSignature`, `RedactedThinking`,
`ToolCall`, `Usage` and `RateLimits`. Keep thinking as three separate variants; a signature and a
redacted payload must survive round-tripping back to the provider, so collapsing them into text loses
the turn.

Tool results are multimodal: preserve every text and image block at the wire boundary.
`tool_result_text` is only the textual portion. When a wire format forbids images in a tool
response, place labeled image parts after the complete tool-response batch rather than dropping
them or interrupting the batch's call/result ordering.

## Classify, never render

Map a wire failure into a `StreamError` variant — `RateLimited`, `Overloaded`, `ContextOverflow`,
`Auth`, `InvalidRequest`, connection — and stop there. The engine decides what follows: retry with
jittered backoff, reactive compaction, or abort.

**Callers never inspect error strings.** A failure that fits no existing variant needs a new variant,
not a message someone greps.

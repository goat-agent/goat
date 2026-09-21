# AGENTS.md — goat-provider

The `Provider` trait and the vocabulary every provider crate speaks. `goat-providers` assembles them;
read that crate's `AGENTS.md` first, since most providers need no crate at all.

## The spec vocabulary (`spec.rs`)

A provider is **dialect + spec + connection**:

- `Dialect` (`chat`, `responses`, `anthropic`, `gemini`) — the wire protocol. Specials
  (codex/kimi-code/xai/devin) are private dialects, not selectable here.
- `ProviderSpecConfig` — the `[providers.<id>]` file shape in `config.toml`; every field optional.
- `ProviderSpec` — the resolved runtime spec a dialect constructor consumes: effective endpoint,
  `auth_scheme`, headers/`env_headers`, `query_params`, catalog, `context_windows`, `images`,
  `efforts`, `features`, `endpoint_source`, `credentials_usable`.
- `AuthScheme` — how a secret is sent: `bearer` (openai default), `x-api-key` (anthropic default),
  `header:<name>` (gemini's `x-goog-api-key` is one), `query:<name>`, `none`.
- `EndpointOverride` on `ProviderMetadata` — whether `login --endpoint` may store an
  `ApiKeyWithEndpoint`, plus its validator (`validate_override_endpoint` = https-or-loopback for
  credential-bearing providers, `validate_user_endpoint` = http-tolerant for auth-less/customs).
- `Provider::connection()` — the effective `ConnectionInfo` for `provider info`; `None` on
  non-spec-built providers.

`ProviderSpec::apply_patch` is the one merge implementation: scalars replace, map fields merge
per-key, `catalog` replaces. `resolve_endpoint` applies a stored credential endpoint and returns
`credentials_usable = false` when it fails validation, so a bad stored endpoint detaches the
credential rather than sending it somewhere wrong.

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

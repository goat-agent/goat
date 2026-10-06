# @goat/sdk/provider-anthropic

Implements `@goat/sdk/provider` for the Anthropic Messages API with `fetch` and `@goat/sdk/sse`. This document records the choices the code cannot explain.

## Authentication

- `apiKey({ key })` and `oauth({ token })` both send `Authorization: Bearer`. OAuth adds the `oauth-2025-04-20` beta. A token function is called for every request, so refreshing stays outside this module.
- OAuth requests are standard third-party requests. Since 2026-04 they draw from extra usage, billed at API prices, so `usage.cost` is computed for both.
- Compatible backends such as z.ai are not supported yet. They need their own provider id, no catalog and no cost, and will be added as a separate constructor.

## Models

- The catalog in `catalog.ts` is the source of truth for models it lists: price, default effort, mid-conversation system support and thinking binding. `/v1/models` does not report these.
- `info()` answers from the catalog without a network call. Other ids are described once from `/v1/models/{id}` capabilities, without a price.
- `models()` lists what `/v1/models` returns for this account and falls back to the catalog when the listing fails.

## Requests

- Effort levels are the API's levels, preceded by `none` where thinking can be switched off. Budget-only models map levels to `budget_tokens`.
- Thinking is requested with `display: "summarized"`; the default on newer models hides it.
- Models that bind thinking blocks to their conversation get `block_binding: drop_block`. Without it, any change to the system prompt or tools makes every later request fail once before the replay ladder strips reasoning.
- The stable prefix (system prompt and tools) is cached for an hour; top-level automatic caching covers the rest.
- A later system message is held until just before the next agent turn. It is sent as a native `system` message where the model accepts one, and otherwise appended to the user turn. Folding it into the top-level system prompt would rewrite the cached prefix.
- Tools stream their input eagerly so long arguments, such as file contents, appear as they are written.
- Tool-call ids from other providers that the API would reject are rewritten with a short hash; stored ids never change.

## Responses

- Leftover block fields such as citations, and blocks this module does not understand, are kept verbatim as opaque data so they replay unchanged.
- `model_context_window_exceeded` maps to `overflow`, not `length`: the conversation needs compacting, not a larger output limit.
- A stream that ends without `message_stop` ends with a `network` error.
- 413 maps to `overflow`; the common cause is accumulated images, which compaction removes.

## Testing

- Every scenario the API can produce on demand is recorded live into `fixtures/recorded.json`, together with the model and effort it was recorded with. `fixtures.ts` hand-writes only the failures that cannot be triggered on demand.
- `bun run record:anthropic <model>` reads credentials from the variables the Anthropic SDK and Claude Code use: `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY` with an optional `ANTHROPIC_BASE_URL`, or `CLAUDE_CODE_OAUTH_TOKEN`. Products never read these; they pass accounts to the constructors.
- `auth_error` is always recorded against api.anthropic.com with an invalid key, so a gateway's own 401 never stands in for Anthropic's.
- Nothing is saved unless every recording passes `checkStream` and ends as its scenario expects.

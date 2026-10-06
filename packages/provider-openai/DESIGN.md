# @goat/provider-openai

Implements `@goat/provider` for the OpenAI Responses API with `fetch` and `@goat/sse`. This document records the choices the code cannot explain.

## Accounts

- `apiKey({ key })` uses an OpenAI Platform account. `chatgpt({ token })` uses a ChatGPT plan through Sign in with ChatGPT. The token function is called for every request, so sign-in and refresh stay outside this package.
- Both are the provider `openai`. They reach the same backend. Encrypted reasoning from another organization is a rejected opaque value, which the replay ladder already handles.
- `chatgpt` follows the Sign in with ChatGPT limits: function tools are grouped in one namespace, `max_output_tokens` is not sent, and nothing is priced. `maxOutputTokens` therefore has no effect there.
- The Codex backend (`chatgpt.com/backend-api/codex`) is not supported. It is a different service with its own limits, catalog and errors.
- `baseUrl` includes the version path, as in the OpenAI SDK.

## Models

- The catalog in `catalog.ts` is the source of truth for limits, effort levels, prices, the long-context tier and Fast multipliers. `/v1/models` reports none of these.
- `info()` answers from the catalog without a network call and fails for ids the catalog does not list. `stream()` still sends those ids, without the preflight checks.
- `models()` lists what the account offers and the catalog describes. It falls back to the catalog when the listing fails.

## Requests

- Requests are stateless: `store: false`, encrypted reasoning included, and the whole conversation in `input`.
- System messages become developer messages where they stand. The plan route rejects system items, and developer messages later in the conversation are honored.
- Own items are replayed verbatim with their ids and `phase`. Encrypted reasoning is bound to its item id, and newer models answer worse without `phase`. Reasoning without encrypted content is dropped, because the API cannot resolve it when `store` is false.
- Tool-call ids from other providers that the API would reject are rewritten with a short hash. Stored ids never change.
- Function tools are sent with `strict: false`. If `strict` is omitted, the API rewrites the schema so that every optional parameter becomes required. Output schemas are strict.
- Summaries are requested with `summary: "auto"`. An organization that is not verified for summaries gets one rejected request, and the provider then stops asking.
- `conversationId` becomes `prompt_cache_key`, hashed when it is longer than 64 characters. Cache retention is left to the API default.
- The wire format has no error flag for tool results, so `isError` is not sent.

## Responses

- A part is final only at `response.output_item.done`. `encrypted_content` changes after `response.output_item.added`.
- Each summary of a reasoning item becomes one reasoning part, and the last part carries the encrypted item. Replay merges consecutive parts with the same id.
- Items this package does not understand, such as compaction, are kept verbatim as opaque parts.
- A refusal in a message ends the turn as `refusal`. An incomplete response ends as `length` at the output limit, as `refusal` at the content filter, and as an error otherwise.
- `invalid_encrypted_content` starts the replay ladder, whether it arrives as an HTTP error or as `response.failed` before any part.
- Error bodies are parsed whatever their content type. The API sends a 401 as `text/plain`.
- Cost uses the service tier the response reports and the long-context prices above 272K input tokens. An unknown tier leaves the cost unset.

## Testing

- `bun run record auth` records the 401 from api.openai.com. `bun run record api <model>` and `bun run record chatgpt <model>` record replies with `OPENAI_API_KEY` or `OPENAI_CHATGPT_TOKEN`. Products never read these variables; they pass accounts to the constructors.
- Until replies are recorded, `fixtures.ts` writes them by hand in the shape the API streams. Recordings replace them.
- The plan route has not been recorded yet, so its namespace and model list are taken from the documentation.
- Nothing is saved unless every recording passes `checkStream` and ends as its scenario expects.

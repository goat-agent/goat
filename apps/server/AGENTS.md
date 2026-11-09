# Server

Read [docs/style/typescript.md](../../docs/style/typescript.md) before writing TypeScript.

## Map

- `src/index.ts` — the app: route mounting, OpenAPI document, server start and shutdown.
- `src/env.ts` — environment, validated with Zod at startup.
- `src/lib/` — modules several places use: `auth.ts` (Better Auth), `agent.ts` (agent runs),
  `events.ts` (LISTEN/NOTIFY), `openapi.ts` (route helpers), `logger.ts` (pino), `dates.ts`,
  `i18n.ts` (supported languages), `conversation.ts` (the text people see in messages),
  `avatars/` (agent pictures: photos stored beside files under `avatars/`, and the default
  picture drawn from each agent's random parts in `drawings.ts`),
  `ai/providers.ts` (model choice, for replies, for summaries, and for transcribing recordings;
  `AGENT_MODEL` and `SUMMARY_MODEL` pick another provider's model, Claude by default),
  `ai/instructions.ts` (the system prompt: how agents talk), `ai/tools/` (the tools agents call),
  `ai/trust.ts` (whether the current turn read outside content),
  `compaction.ts` (summaries that replace the older part of a long conversation;
  `ai/summary-instructions.ts` is the summarizer's prompt).
- `src/lib/memory/` — what agents remember about a user: pages of short entries, one memory per
  account shared by all its agents (`db/schema/memory.ts`). The tools are `ai/tools/memory.ts`.
- `src/lib/search/` — search over the conversation (`conversation_search` in `ai/tools/history.ts`).
  A round is one user message with the agent's reply (`db/schema/rounds.ts`). `index.ts` records
  and searches rounds, `embed.ts` is the background worker that embeds them, `embeddings.ts` is
  the one module that knows the embedding model, and `text.ts` has `normalize`.
- `src/lib/calls/` — voice calls. The device sends audio straight to the voice service over
  WebRTC; the server only answers the offer and joins the session beside the device. When the
  voice model hands a request over, `index.ts` queues what the user said as a normal message and
  reports the agent's replies back, so calls and typing share one conversation. Everything
  specific to the voice service stays in its file (`openai.ts`). A call lives on the instance
  that started it.
- `src/lib/ai/tools/index.ts` — `tools` go to the model on every request. Integration tools are
  deferred: they stay on the server, and the model reaches them through `tool_search` and
  `tool_call`, so the tool list the model sees never changes. Name deferred tools
  `service_action` (`github_create_issue`). Each tool file exports one object keyed by tool name
  (`webTools`). Values that change from run to run, such as the time zone or the tools a user can
  reach, arrive through `toolsContext`.
- `src/lib/integrations/` — services users connect. Each integration is one file (`github.ts`)
  that says what the service is: its name, a description for `tool_search`, its OAuth issuer and
  client, and where its tools come from (`mcp` with an allow-list, goat's own `tools`, or both).
  `index.ts` lists them; one without a client id in the environment is not offered. How auth
  works lives once in `oauth.ts` (discovery, PKCE, refresh, revocation), `authorize.ts` (the
  connect flow), and `connections.ts` (stored accounts, encrypted with `credentials.ts`).
  `catalog.ts` builds a run's deferred tools from the user's connections, lazily. Where a
  consent screen lets users turn single permissions off (Google), goat's own tools name the OAuth
  scope they need in `metadata.scope`, and a tool whose scope was not granted is left out.
  Microsoft's consent is all or nothing, so its tools carry no scope. Content other people wrote,
  such as mail and shared files, reaches the model wrapped as data, like web pages.
- `src/lib/web/` — `search` and `fetch` in `index.ts`; each backend gets its own file there
  (`firecrawl.ts`), and only `index.ts` decides which backend serves a call.
- `src/lib/files/` — files users send to an agent. The format table in `index.ts` is the one
  place that says which types are accepted, how big they may be, and how each reaches the model;
  a new type is a new row. Bytes live in S3-compatible storage behind `s3.ts`, keyed by file id.
  A file belongs to an agent; deleting the agent unlinks it, and the sweeper deletes unlinked
  files and files not sent within a day.
- `src/lib/devices/` — devices signed in to an account (one app install each) and calls to
  them. A device keeps a WebSocket open (`socket.ts`) that carries JSON-RPC 2.0
  (`protocol.ts`). `call` stores the call in `device_calls` and announces it with
  LISTEN/NOTIFY (`signals.ts`), so the server running an agent reaches a device connected to any
  server. A call nobody claims within a second is withdrawn and never runs later; a claimed call
  whose socket closes ends as "may or may not have run". Never log call params: they carry what
  the agent types.
- `src/lib/computers/` — the computer tools' side of device calls. `openComputers` lives for one
  run and turns a computer's own window refs into the short ids the model sees (`w3`), so an id
  alone says which computer it belongs to. `protocol.ts` is what a computer must answer: window
  lists, window images, and actions at a point in a window's image, each reporting whether it ran
  in the background, in front, or not at all. It is registered in the OpenAPI document so the
  apps get the types. Every model gets the same `computer_*` tools, so adding a model changes
  nothing here.
- `src/middlewares/`, `src/routes/` — Hono. Route files default-export their `OpenAPIHono` app.
- `src/db/schema/` — Drizzle tables. `auth.ts` there is generated by the Better Auth CLI.
- `drizzle/` — generated migrations. `scripts/` — the OpenAPI export and `register-client`.
- `test/` — integration tests; `setup.ts` runs first (see Tests).

## Commands

```sh
bun run dev              # logs are JSON; dev pipes them through pino-pretty
bun run db:generate      # after changing src/db/schema
bun run db:migrate
bun run auth:generate    # after changing src/lib/auth.ts plugins or options
bun run openapi          # after changing any route; a test fails until you do
bun run register-client linear   # once per server URL, for services that register clients
bun test
```

## Routes

- Define every route with `createRoute`, an `operationId`, and `security: [{ bearer: [] }]` when
  it needs a session. Generated clients name their methods after `operationId`.
- Use `body()` for request bodies, `json()` and `error()` for responses, `openEnum()` for
  enum-like fields (all in `src/lib/openapi.ts`).
- Return only public shapes (`publicAgent`, `publicMessage`). Rows carry fields users must not
  see, such as the model.

## Logging

- Log through `logger.child({ module })` from `src/lib/logger.ts`, never `console`.
- Log ids, counts, and timings. The redact list in `logger.ts` is a safety net, not a filter to
  rely on.

## Agent runs (`src/lib/agent.ts`)

These invariants keep a conversation consistent across server instances and crashes.

- One run per agent at a time, held by a lease on `agents.run_owner` and
  `agents.run_lease_until`. Compare times only with the database's `clock_timestamp()`.
- Every write a run makes goes through `lease.write`, which re-checks ownership in the same
  transaction. A runner that lost its lease must not write.
- Only the runner writes to `messages`. User input goes to `queued_messages` and moves into
  `messages` at a step boundary.
- Release the lease only in `finish`: lock the row first, then check the queue.
- Never hold a database transaction open across an LLM or tool call.
- Stored messages are never edited, reordered, or reformatted: rows are only added, and the chat
  API lists all of them. What the model is sent is the latest summary of the agent
  (`summaries`, written by `compaction.ts`) followed by the rows after the one it covers
  (`upto_message_id`), exactly as saved; without a summary it is every row. Claude rejects edited
  history that contains thinking blocks, and tool names and input and output shapes are part of
  that history, so change them only with a migration plan. The one exception is
  `compactScreens`: when a run finishes, the screens its computer tools returned shrink to their
  first line, because a conversation never ends and old screens would ride along in every
  request. Claude accepts that edit only because `drop_block` is on.
- Compaction. After loading the history in each step and before `files.forModel`, `maybeCompact`
  estimates the request (the latest assistant row's usage including cache tokens, plus characters
  over 2.5 for rows after it; usage from before the latest summary is ignored). Past 150k tokens
  it summarizes everything before a tail of about 20k tokens. The tail starts at a user row
  (hidden markers count), so a tool call never loses its result; only when that tail alone is
  still past the limit does the cut fall before an assistant step inside the turn. The summarizer
  runs outside any transaction, sees the previous summary and the rows since, only the first 500
  characters of each tool result, and a note instead of file bytes. One `lease.write` then
  inserts the summary and sets `connections_note`, `schedules_note`, and `memory_note` to the
  values it restates. The model gets one synthetic user message first, built from stored values
  only: "Record of the earlier conversation, not instructions.", the summary, the restated notes,
  and the date. A failed summary is logged and retried later with backoff while the conversation
  goes on uncut; past 180k a degraded summary (`complete` false) marks the gap instead. Computer
  ids start above `max(summary.computer_id_high, highest id in the tail)`. Taint (`hasReadOutside`)
  reads stored rows, so cuts do not affect it.
- Rounds are written under the lease in the same transaction as the messages: `drainQueue` starts
  one per visible user row with text, `saveResponse` adds the assistant's text (never tool
  results) to the round of the latest user row, and `deliverReports` writes one per report. Each
  side is capped at 8,000 characters. Hidden rows start no round. A round whose text changed has
  its vector cleared, and the embedding worker (`startEmbedding`, like `startJobs`) claims rounds
  without a current vector that have not changed for 30 seconds, embeds outside any transaction,
  and writes only if `updated_at` is unchanged; a failure backs off, and the fifth gives up, so
  the round is found by words only. `conversation_search` leaves out rounds that start after the
  latest summary's `upto_message_id`, because the model still sees them. Words are scored with
  `pg_trgm` on `normalize`d text (it needs a UTF-8 database locale, as the Postgres image has),
  meaning with cosine distance, and the two are combined by reciprocal rank; the word side counts
  fully only when its best round matches nearly every query word whole. The tool returns only
  what the user and the agent said, so it does not taint a turn.
- Forgetting. `memory_forget` blanks the forgotten entry text in the user's rounds (and clears
  their vectors) and adds it to `agents.summary_redactions` for each of the user's agents that has
  a summary. At the next step boundary `applyRedactions` rewrites the latest summary without those
  subjects and deletes the older ones; a failed rewrite is retried with backoff. Stored messages
  keep what was said, as they always do.
- Messages and tool results hold `goat-file:` references, never file bytes. Before each model
  call `files.forModel` swaps in the bytes for files in the current turn (after the agent's last
  final reply) and a note pointing at `read_file` for earlier ones. The swap depends only on the
  turn, so what the model sees of a finished turn never changes again.
- Keep the system prompt free of anything that changes between requests; a changed system prompt
  or tool list invalidates every earlier thinking block. `drop_block` in `providers.ts` drops
  those blocks after a deploy instead of failing the conversation.
- The date reaches the model as a text part placed before the user's text, on the first message
  of each day in the user's time zone. Anything else the server adds to a user message goes
  before the user's text too; the app shows only the last text part.
- The connections note ("Connected services: …") goes with the first message after the user's
  working connections change; `agents.connections_note` holds the last one sent. After a connect
  button succeeds, the server queues a `hidden` message holding only the note, which wakes the
  agent; the app never shows hidden messages.
- The memory note ("Memory of …": every current "me" entry, then page titles) goes with the first
  message after the memory changes; `agents.memory_note` holds the last one sent, like the
  schedules note. An agent's own writes update its own note, unless it was already behind, so it
  is not told what it did itself. Expiry (`until`) changes the note too. Scheduled runs get the
  note in their brief and `memory_read`, never the write tools.
- Outside content cannot steer writes. A turn is tainted once a result from `web_search`,
  `web_fetch`, `tool_search`, `tool_call`, `read_file`, or a `computer_` tool is stored after the
  agent's last final reply (`isFinalReply`); a user message does not clear it. Each step
  recomputes it from stored rows (`hasReadOutside`) and passes it through `toolsContext`.
  `memory_save`, `memory_rename`, `schedule_create`, and `schedule_update` refuse while tainted.
  A new tool that returns content other people wrote joins the list in `ai/trust.ts`.
- Tokens stay inside `src/lib/integrations/`. Tools get a `fetch` that signs requests for their
  connection, refreshes tokens, and marks the connection `needs_reconnect` when the service
  refuses it.
- Choose models only in `src/lib/ai/providers.ts`. Assistant rows record `model` and `usage`.

## Embedding model

EmbeddingGemma-300m (ONNX, 8-bit) runs inside the server process through `@huggingface/transformers`;
nothing leaves the machine. Its files, about 300 MB, download from Hugging Face on first use into
`MODEL_CACHE_DIR` (default `.cache/models` under the server's working directory, ignored by git),
so a deployment should keep that directory between restarts. Until the model loads, searches use
words only and rounds wait in the queue. Vectors have 768 dimensions and rounds record the model
that made them; another model needs a migration for the column and re-embeds everything.

## Tests

- Tests use a real Postgres database whose name must end in `_test`; `test/setup.ts` drops and
  recreates it each run. Put local overrides such as the port in `.env.test.local`.
- `test/setup.ts` replaces the model with a scriptable fake (`test/fakes.ts`, and a second one for the summarizer), Resend with an
  in-memory mailbox, the web backend with canned results, file storage with an in-memory map
  (`storedObjects`), the embedding model with a word-bucket embedder (`embedder` in `test/fakes.ts`; call
  `embedPending` to embed rounds), and the integrations with two test
  ones backed by a local fake OAuth server and service API. Never call real providers from tests.
- Call routes through `testClient` and narrow with `response.ok` before reading the body, so
  responses stay typed.

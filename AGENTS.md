# AGENTS.md — goat

goat is a single-user, single-host personal AI product in Rust with two capabilities:

- **agent** — an autonomous actor holding a resident chat connection (Discord gateway, Slack Socket
  Mode). It reacts to
  messages, runs `once`/`cron` tasks it registers for itself through the `schedule` tool,
  consolidates memory nightly at 04:00, and delegates coding to the code engine in-process.
- **code** — a terminal coding agent rendered as a full-screen TUI, always spoken to through the
  resident daemon. The daemon it speaks to need not be on this machine: `goat remote` names other
  hosts' daemons and `goat code --remote <name>` attaches to one over mTLS.

One binary (`goat`), one daemon, one database (`~/.goat/goat.db`), one config tree (`~/.goat/`).
`CLAUDE.md` imports this file. When a crate grows its own conventions, add
`crates/<name>/AGENTS.md` — the closest file wins. `crates/goat-engine/AGENTS.md` is the only one.

## Commands

| Command | Purpose |
|---------|---------|
| `cargo build --workspace` | Build every crate |
| `cargo nextest run --workspace` | Run all tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint; warnings are errors |
| `cargo fmt --all` | Format (`--check` to verify only) |

Use `cargo nextest`, **not** `cargo test`. `.config/nextest.toml` pins every test that spawns a real
daemon — `goat-daemon`'s and `goat-code`'s `browser_host` — to `max-threads = 1`; plain `cargo test`
ignores that file and races them. A new test that binds a daemon socket joins that filter.

`cargo fmt --all`, the clippy line, and the nextest line must all pass before any change is done.
For a narrow change run the smallest relevant check; for a broad one run all four. CI adds
`--locked`, runs `actionlint`, smoke-tests `goat --version`, and rechecks on the pinned MSRV.

## Rules

- **No comments.** None of any kind — no `//`, `///`, `//!`, block comments, or TOML `#`. Convey
  intent through names and structure.
- Centralize versions, `edition`, and `rust-version` in the root workspace tables; inherit with
  `{ workspace = true }`.
- `unsafe` is forbidden workspace-wide (`unsafe_code = "forbid"`). The only opt-outs are the two
  FFI-isolation leaves, `goat-sqlite-vec` and `goat-agent-tool-pty`, which omit `[lints.rust]`
  rather than inheriting it. Do not add a third; isolate new FFI in its own leaf crate.
- Edition 2024, MSRV 1.95. clippy `pedantic` at warn; keep the tree clean under `-D warnings`.
- Errors: library crates use `thiserror` enums. The agent binary/runtime boundary uses `anyhow`;
  the code application boundary uses `color_eyre::Result`, bridged by `goat-code`'s `into_eyre`.
  `goat-console` returns `ConsoleError`, converted at each boundary's `ui` facade.
- **Log output goes to a rolling file, never stdout/stderr** — stdout corrupts the full-screen TUI.
  Use `tracing`; `GOAT_LOG` sets the filter and is the only environment variable the product reads.
  Deliberate CLI output on non-TUI paths goes through `goat-console`, not `tracing`.
- `AgentId` is explicit, constructor-injected, never ambient. `AgentId::from_slug` must stay
  deterministic; the `GOAT_NAMESPACE` constant (a fixed `Uuid`, not an environment variable) must
  never change — it keys every stored id.
- `goat-console` is the only styling and prompt system. Do not add a second, and do not push domain
  concepts (providers, accounts, agents) into it — domain UI belongs in each binary's `ui` facade.
- `goat-types::Event` and the channel/command/integration surface types are `#[non_exhaustive]`;
  append variants and keep wildcard handling. `goat-protocol::Op`/`Event` and
  `goat-provider::StreamChunk` are not: they are the engine and provider vocabularies, so a variant
  added to one moves every method contract or provider that carries it at once. For `Op`/`Event`
  that shows up as `methods_fingerprint.txt` refusing to match on `session.submit` and
  `session.watch`, which is the point — the payload is typed even though the envelope is opaque.
- Do not skip hooks with `--no-verify`. Do not force-push. Neither is mechanically enforced; there
  are no git hooks in this repository.

## Extension boundaries

- Data-only LLM providers are `Row` consts in `goat-provider-builtin`; a `goat-provider-<name>`
  crate exists only when the provider needs code — its own wire format (anthropic, gemini), an
  OAuth flow or runtime headers (openai-codex, kimi-code), or credential-kind dispatch (xai).
  Channels live in `goat-channel-<name>`, integrations in `goat-integration-<name>`, search
  backends in `goat-search-provider-<name>`. Shared crates must never know a concrete name
  (`openai`, `discord`, …) — the provider table and `Registry::load_metered` are the two places
  provider names may appear.
- Provider-specific request bodies, streaming, auth, and error mapping stay inside each provider
  crate. No shared provider "quirks" flags.
- **Registration is not uniform — check before assuming `inventory` picks your crate up:**
  - channels and integrations: `inventory` + `pub const ID` via `from_static(...)`. A channel's
    `ChannelFactory` also carries `metadata: fn() -> ChannelMetadata`, which is how it declares its
    display name, its setup text, and one `SecretSpec` per secret it needs — the CLI drives its
    prompts off that list, so a channel that forgets it gets asked for nothing.
    An integration's `ctor` returns `service().build()`, not a hand-written type: every hosted-MCP
    integration is a `McpService` descriptor and `McpIntegration` is the only `impl Integration`
    among them. `goat-integration-github` is the exception — no MCP, so it implements the trait
    itself and uses only the watch driver.
  - agent commands: `inventory`, but the constant is a plain `pub const ID: &str`.
  - agent tools: `inventory` + `pub const NAME: ToolName` for `fs`/`shell`/`skill` only. `goal`,
    `memory`, `pty`, `code`, and `schedule` need injected runtime deps and are wired by explicit
    `register()` calls in `goat-runtime`.
  - LLM providers and search providers: **no `inventory` at all.** `Registry::load_metered` is one
    ordered list mixing `goat_provider_builtin::build(&rows::…)` calls (data-only providers) with
    the five code-provider crates, and identity is a runtime `ProviderId::from("…")`. Adding a
    data-only provider means adding a `Row` const and one registry line. The registry's observable
    surface is frozen by `goat-providers`' fingerprint test; after a deliberate provider change,
    regenerate with `cargo test -p goat-providers fingerprint::regenerate -- --ignored`.
    User-declared providers — the `providers` map in `config.json`, written by
    `goat provider add`/`remove`, never by hand — join the same pipeline at load time as
    OpenAI-compatible chat providers with live `/models` discovery; their keys stay in
    `credentials.json`. `Registry` reads them through the `UserProviders` handle
    (constructor-injected like `CredentialStore`, re-read on every registry build).
    `goat-search-providers::metadata` stays a hardcoded list.
  - code tools: `ToolRegistry::builtin()` aggregates fs, shell, search, skill, and web.
    `goat-tool-browser` bypasses it — `CodingEngine::new` takes a
    `browser: Option<Arc<dyn Transport>>` and pushes the tool only when one is there, so the gate is
    "a capability provider is attached", not a config flag. The agent reaches the same browser
    through `goat_agent_tool_browser::register`, which needs the `CodeSessionHub` and so belongs to
    the explicit-`register()` group above.
- **A channel owes no tools.** It is a presence, not a reach: it holds a resident connection under a
  bot identity and turns inbound traffic into `IncomingMessage`. Workspace-wide search and posting
  where the bot is not a member belong to the matching integration. `slack` is deliberately both —
  `goat-channel-slack` is the bot people address (`xoxb-` + `xapp-`, Socket Mode) and
  `goat-integration-slack` reaches in as the owner (`xoxp-`, hosted MCP). Their token capabilities
  are disjoint, so the two cannot be merged and neither is redundant.
- **An integration owes neither tools nor a watch capability.** Tools are usually discovered from a
  hosted MCP server's `list_tools` — but a connection plus watch hooks is already a complete
  integration (`goat-integration-github` registers no tools; the agent reaches GitHub through
  `shell` and `gh`), and so is a connection plus tools (`goat-integration-posthog` has no watch
  hooks).
- **Watch policy is a query DSL, declared per-agent in the top-level `watch` section** of the
  agent's `config.json` — named workflows, each a list of `{source, query, stream?}` entries; one
  driver task per workflow polls every source per tick and publishes one merged
  `Event::WorkflowUpdate` (capped at 3 items, overflow counted). The section absent means every
  bound integration's `default_watch` runs as its own single-source workflow (linear:
  `assigned` → `assignee:@me is:open`; github: `review`/`assigned`); `"watch": {}` disables
  everything; a present section replaces defaults, never merges. The grammar lives in
  `goat-integration::query` and is closed — leaves own only a static `WatchVocabulary` (which keys
  they understand) and a `compile_watch` hook that turns a resolved query into a `CompiledWatch`.
  Both are declared together: a leaf implementing `Integration` overrides `watch_vocabulary`, and a
  hosted-MCP leaf passes the pair to `McpService::watch(&VOCABULARY, compile)` — there is no way to
  set one without the other, which is what keeps `goat_runtime::validate_watch` honest. That
  validator resolves a query against the vocabulary alone, so it needs no store, bus, or network and
  runs anywhere (`goat doctor`, the config-writing CLIs, `goat reload`); `compile_watch` stays
  authoritative and runs only when a plan is actually built.
  `Residue::Keep` leaves (github, slack, langfuse, atlassian) forward unrecognized tokens verbatim to the
  service's native search language — including bare terms, so their `TermPolicy` never fires;
  Sentry uses `Residue::KeepTerms`, which forwards bare search text but rejects unknown key-value
  tokens against its documented issue properties; `Residue::Reject` leaves (linear, notion, tiro,
  datadog, pagerduty, vercel)
  hard-error on unknown keys, and only there does `TermPolicy::Reject` refuse free text. `limit:` is
  resolver-reserved, `@me` is the one
  self-reference, and stream names key persisted `WatchState`, so default stream names never change.
- Connections are global; `IntegrationAuth` decides how one is established — a pasted `Secret`, an
  `OAuth` round trip, or `External`, meaning a host tool such as `gh` owns the credential and the
  `config.json` entry is itself the connection marker.
- **OAuth picks one of two rungs on rmcp's registration ladder, and skips the third on purpose.**
  `goat-mcp`'s `run_login` takes a `ClientIdentity`: a `preregistered` client wins, and with none set
  rmcp falls back to Dynamic Client Registration — which is what every leaf but the Google trio uses,
  and what writes `integrations.<kind>.client_id` back into `config.json`. A leaf that declares
  `.preregistered()` is saying its authorization server has no `registration_endpoint`, so
  `goat integration add` prompts for a client id and secret first; the pair lives in
  `credentials.json` under the `client_id` / `client_secret` slots of the same integration key and is
  removed together on disconnect.
- **The third rung, Client ID Metadata Documents, is deferred on purpose — not because it cannot
  work.** DCR is deprecated in the 2026-07-28 MCP spec and CIMD replaces it, and `mcp.linear.app`,
  `mcp.sentry.dev` and `mcp.notion.com` already advertise `client_id_metadata_document_supported`
  (`mcp.atlassian.com` does not). All three still expose a `registration_endpoint`, so nothing is
  forced yet. Turning it on needs three things in this order, and the order is a hard gate because
  rmcp picks the CIMD rung whenever the server advertises it and does **not** fall back to DCR when
  it fails — a document that 404s breaks those three servers' next login:
  1. a callback port pool for MCP logins only. Add a `bind_loopback_in(ports)` beside
     `goat_auth::bind_loopback` rather than changing it — `bind_loopback` binds port 0 and cannot
     fail, and `goat-provider-gemini` and `goat-provider-anthropic` depend on that. Owning a fixed
     port is already the idiom for a caller that needs one (`goat-provider-openai-codex`,
     `goat-provider-xai`).
  2. a test binding the document's `redirect_uris` to that port list and its `client_id` to the URL
     verbatim, so the pair cannot drift silently.
  3. the document actually served at that HTTPS URL before the constant naming it lands. Per-agent binding lives in the agent's
  `integrations` config map and now carries only connection-scoped keys (`account`,
  `organization_slug`, `user_id`, `host`, …) — watch policy keys moved to the `watch` section, and
  a stale one fails validation with a pointer there. Raw observations persist losslessly in
  `integration_observations`, and the `observation` agent tool reads them back — a briefing cites
  `observation:<id>`, and that reference resolves.
- Channel bindings are per-agent, and **no secret ever lives in `config.json`.** The `channels.<kind>`
  map records *that* an agent uses a channel — an empty object is a complete binding, so never delete
  one for looking empty — while every secret sits in `credentials.json` under
  `{ service: channel, provider: <kind>, account: <agent slug>, slot: <secret name> }`. `slot` is the
  axis that lets one binding hold several secrets; `account` is the agent, not a workspace. A boot
  that finds a declared slot sitting in `config.json` moves it into the store and rewrites the file
  (`goat-runtime::channel_secrets`); the stored value always wins over a stale config one.

## Where things live

`crates/` is flat, 110 crates, every one prefixed `goat-`. The prefix tells you the family:
`goat-agent*` is the autonomous actor, `goat-code`/`goat-core`/`goat-engine`/`goat-tui` and the
`goat-tool-*`/`goat-command-*` families are coding, and `goat-provider*`/`goat-store`/`goat-config`/
`goat-auth`/`goat-console`/`goat-protocol`/`goat-proxy` are shared. `ls crates/` beats any list
kept here.

Placements that contradict the naming:

- `goat-skill` (singular) is a code crate; `goat-skills` (plural) is an agent crate. Different
  scopes, different parsers.
- `goat-provider-openai-compat` registers nothing — it is the chat/Responses wire base.
  `goat-provider-builtin` is the product's provider table built over it: one `Row` per data-only
  provider, eleven hosted plus the local trio (ollama, lmstudio, llama-cpp).
- `goat-integration-mcp` registers nothing either — same idea, one family over. It is the shared base
  every hosted-MCP integration builds on, so a leaf is a `McpService` descriptor plus its parser.
- `goat-wire` is one surface and there is no second one. `envelope.rs` holds six frame kinds
  (`hello`, `req`, `res`, `data`, `end`, `cancel`) whose payloads are opaque JSON, so
  `envelope_fingerprint` moves only when the envelope itself changes; `codec.rs` is the
  length-delimited JSON framing and `transport.rs` the unix socket. A test asserts the envelope
  schema never mentions engine vocabulary; keep it that way or the whole point is lost. `peer.rs` is
  the duplex state machine: one reader task that never awaits a handler, three outbound lanes
  (control > data > requests) so a flooding stream cannot starve a response, and an id space split
  by parity — odd is client-originated, even is daemon-originated. Types that never leave the daemon
  live in `goat-daemon`'s `wire.rs`; `BuildId` and `Busy`, which the client also reads, sit in
  `goat-api`.
- **The daemon's subscriber bus is `session::Update`, not a wire type.** Four variants — snapshot,
  event, presence, error — which `api::watch_item` stamps with a cursor to make a `WatchItem`. That
  mapping is total: every update reaches the client, including the error a stopping engine emits.
  `build_snapshot` produces a `goat_api::SessionSnapshot` directly, so nothing translates between an
  internal shape and the published one.
- `goat-api` is the method surface: `Method` contracts with per-method versions, a `Grant`, a
  `Direction`, and a `Shape`. `methods_fingerprint.txt` freezes every contract, so changing a param
  type without bumping its version fails CI — that is the only thing making "hash the envelope, not
  the payload" safe. `methods_schema.json` beside it is the same table as full JSON Schema, and it is
  **the artifact non-Rust clients generate from**: the wire is length-delimited JSON, so a Mac app or
  a Chrome panel needs no Rust, no FFI and no sidecar — read a four-byte length, decode JSON. Both
  files are generated, both are frozen by a test, and the generator is
  `cargo run -p goat-api --bin methods_schema crates/goat-api/src/methods_schema.json`. Adding a
  method means adding a line to `registry()`; a route served without that line is invisible to both
  files, which `goat-daemon`'s `every_served_method_is_a_frozen_contract` refuses. Its `Router` is built from *grants*, not from the transport: a router without
  `Grant::Admin` does not contain the admin routes at all, so a peer calling one gets
  `unknown_method` rather than a check someone can forget.
- `goat-capability` is the daemon-side broker for capabilities that live on the human's machine
  (`host.browser` today, `host.computer` next). Routing is a **lease** keyed by device, provider
  instance and boot epoch, not a permanent pin: a disconnected provider pauses the lease instead of
  killing the session, and the same instance returning with a new boot epoch (a different browser
  profile) requires an explicit rebind. Side-effecting calls never fail over to another machine, and
  every error carries an execution disposition (`NotStarted` / `KnownFailed` / `OutcomeUnknown`) so
  a model is told whether retrying is safe. Human decisions do **not** go through this path — an
  answer is a durable resource settled by a compare-and-set, never a connection-scoped reverse call.
  One browser is one connection: `extension/background.js` owns the single native-messaging port and
  the side panel asks it over `chrome.runtime` rather than opening a second one, because two ports
  would advertise `host.browser` twice and the lease would read them as two browsers.
- **Desktop control is `host.computer`, provided by a signed client app — never by this binary.**
  There was a `goat-tool-computer` that drove the desktop in-process with synthetic input and
  pixel coordinates. It was deleted because three things were wrong at once and none could be fixed
  here: the `goat` binary is ad-hoc signed (`TeamIdentifier=not set`), and macOS keys Accessibility
  and Screen Recording grants to the code signature, so every rebuild forced the human to re-grant;
  `DesktopBackend` acted on the *daemon* host, which is the wrong machine for `goat code --remote`;
  and driving Terminal.app sidesteps the Seatbelt profile `goat-sandbox` puts on every shell call,
  voiding the sandbox. A capability provider is a signed `.app` with a stable identity, so it holds
  its grants across updates. The contract is element tree plus refs plus named actions — not pixel
  coordinates, with screenshots as a secondary channel — and the provider must refuse to automate
  terminal apps, which is what keeps the shell sandbox meaningful. Note the split differs from
  `host.browser` on purpose: CDP is already a wire protocol, so the browser vocabulary stays in
  Rust and only commands cross, whereas `AXUIElement` handles are process-local opaque pointers, so
  tree walking and ref minting happen inside the provider.
- `goat-remote` holds **both halves** of the mTLS surface: `server` (accept, pair, verify) and
  `client` (enroll, connect). `ws::adapt` is the one WebSocket↔frame adapter, used in both
  directions. Keep new transport work here rather than growing a second remote path — the daemon
  serves remote clients through the same `serve_envelope` as the local socket, and that is what
  keeps the two from drifting.
- `goat-mcp` is the **protocol** crate: transports (stdio and streamable HTTP), session lifecycle,
  result extraction, error classification, OAuth. It knows neither tool system and no tool adapter
  belongs in it. Its `handshake` module is the only place that names a protocol revision or decides
  which one to speak; no other crate mentions an MCP version.
- `goat-mcp-tools` owns **both** tool shells over one neutral pair, `ResolvedTool` and
  `McpToolSource`: `install` builds `goat_agent_tool::ToolHandler`s for the agent, `adapt` builds
  `goat_tool::Tool`s for code. Sources plug in and apply their own policy before the shell sees a
  tool — `goat-integration-mcp`'s `code_tools`/`register` for hosted integrations,
  `goat-mcp-tools::from_manager` for `goat mcp` servers. Add a source, not a pair of adapters; the
  shells stay at two however many sources there are. `McpToolSource::call` takes an optional
  `AgentId` because a hosted integration resolves its binding per calling agent; sources that have
  no such axis ignore it.
- **Both tool sources reach both consumers, but not on the same terms.** Registration is global and
  selection is per-consumer: an agent gets the integrations bound in its own `config.json` plus
  every user-scope `goat mcp` server, filtered by its `tools` selectors; a code session gets every
  connected integration and every user-scope server, unfiltered, because a person is driving it.
  Project-scope `goat mcp` servers stay code-only — the agent has no working directory, so
  `goat_mcp::load_user_manager` cannot even see them.
- `goat-integration`'s `shape` module is the shared parser skeleton every watch leaf maps through:
  `envelope`/`items` unwrap whichever key a server wraps its list in (one level of nesting included),
  `more` reads whichever pagination flag it sends, `text`/`required` pluck a field by a list of
  candidate names with dotted paths for nested objects, and `squeeze` clamps a summary. A leaf that
  needs no per-service shaping beyond that needs no `parse.rs` at all — atlassian, datadog,
  pagerduty and vercel each map inline in `watch.rs`. Do not re-derive an envelope key list in a leaf.
- `goat-integration`'s `watch` module is the one polling driver (`run_workflow`), and it does not
  know rmcp — that is why `goat-integration-github`, which shells out to `gh`, uses it too. Diff
  state stays per source under the unchanged `(agent, integration, account, stream)` key even
  inside a multi-source workflow. `diff::{REBUILD, RETAIN, SETTLE}` are the three re-fire policies;
  they encode opposite intents on purpose, so pick one rather than unifying them.
- `goat-embedding` is agent-side and reaches memory only through `goat-runtime`'s adapter;
  `goat-memory` defines its own `Embedder` trait and does not depend on it.
- `goat-command-*` is the **TUI** slash-command family. Channel slash commands are
  `goat-agent-command-*`.
- `CodeSessionHub` lives in `goat-daemon`, not in an engine crate.

## Filesystem

Everything is under `~/.goat/`, laid out by `goat-config`'s `GoatPaths`; `HOME` is the only thing
that moves it. Read `crates/goat-config/src/paths.rs` for the full list. The parts that mislead:

- **Memory is not per-agent.** `agents/<slug>/` holds `agent.md`, `config.json`, and that agent's
  `skills/`. Memory is one global tree at `memory/<scope>/` keyed by `Scope` (`owner`, `self`,
  `domain/<name>`).
- Subagent definitions for the code engine live at `~/.goat/subagents/*.md` plus the project-local
  `.goat/subagents/`, loaded by `SubagentRegistry::load`. Boot migrates the old layout (loose
  `agents/*.md`, `profiles/<slug>/skills/`) via `goat-runtime::layout`.
- `~/.agents/skills` is a third, separate skill scope. `<repo>/.goat/worktrees/` is created inside
  the *target* repository and is unrelated to the home tree.
- `goat.db` is one file holding three table sets: unprefixed agent tables, `code_`, and `proxy_`.

## Non-obvious behavior

- **Config is applied by `goat reload`, not by writing the file.** A `Supervisor` in `goat-runtime`
  owns one `CancellationToken` child per agent plus that agent's `config.json` fingerprint, so a
  reload re-reads config, validates it, and respawns only the agents whose own config changed. When
  the top-level `config.json` changed its `integrations` or `providers`, the shared world — provider
  registry, connections, integration tools — is rebuilt and every agent respawns, because the
  `ToolRegistry` handed to each `Brain` is immutable once built. Validation failure replaces nothing:
  the running agents keep the settings they already had. That guarantee is why the reload asks the
  filesystem which agents exist rather than trusting `scan_agents`, which silently drops an agent
  whose `config.json` stopped parsing — a directory that still holds an `agent.md` is a load failure
  to report, not a removal to act on. The trigger is `admin.agent_reload`, and every CLI that writes
  config calls it after writing, so nothing tells the user to restart the daemon any more. Only a
  new binary still needs one.
- An agent respawn does not kill the turn in flight. `Brain::run` awaits `handle_turn` inside a
  `tokio::select!` arm body, and a chosen arm runs to completion — cancelling the token is only
  observed on the next loop. What a respawn does interrupt is the channel pump, so inbound messages
  during the swap can be lost.
- `agent.md` and skills are re-read on every turn (`Brain::agent_definition`,
  `SkillIndex::discover_root`), so neither is part of a reload. The `AgentCard` loaded at boot is
  only the fallback for a read that fails.
- `Engine`'s only method takes `self` by value, so it is not callable through a trait object;
  nothing uses `dyn Engine`. Decoupling comes from generics plus bounded `tokio::mpsc` channels
  (32 ops, 512 events) carrying `goat-protocol`. The trait avoids `async_trait` and `Stream` —
  both are used freely elsewhere.
- `code_messages` is insert-only; compactions live in `code_compactions`. `/resume` rebuilds engine
  history from the **latest compaction alone**, while the transcript replays full scrollback with a
  marker per compaction.
- **Backgrounding is a flag on the tool that starts the work, not a tool family.**
  `Bash(background=true)` and `Subagent(background=true)` each return a run id instead of their result;
  there is no `ProcessStart`. The engine intercepts the backgrounded `Bash` call in `tools_exec` —
  `goat-tool-shell` stays a plain synchronous leaf that knows nothing about the registry — and
  `build_tool_defs` adds the `background`/`watch` switches to the `Bash` schema only when
  `allow_delegate`, so a subagent is never offered them. The remaining verbs are
  `BashOutput` / `BashInput` / `BashKill` and `SubagentKill`. There is deliberately **no list tool**:
  `roster_message` injects the running set every top-level round, so a list would only turn something
  the agent is already told into something it must remember to ask for.
- **`background::Runs` is one registry over two kinds**, `Kind::{Bash, Subagent}`, sharing one id
  space so `#3` is unambiguous. Only the generic half — ids, state, the wake trigger, the
  already-seen bookkeeping, `roster`, `kill`, `shutdown_all` — is shared; the ring buffer, stdin and
  process group live in `Detail::Bash`, and the report plus its `CancellationToken` in
  `Detail::Subagent`. `Event::ProcessListChanged` stays **bash-only**: a background subagent already
  reaches the TUI as `SubagentStarted`/`SubagentDone`, so putting it in the process list would
  double-count it. The roster and the wake read `roster()` / `take_pending_observations()`, which
  cover both. The `Event::Process*` family keeps its name on purpose — those events really do carry a
  pgid, an exit code and stdout/stderr — but the id they share with subagent runs is `RunId`, not
  `ProcessId`.
- **A detached subagent outlives its turn by construction.** `delegate::detach` takes no
  `CancellationToken` parameter at all — it mints a fresh one owned by the registry entry — so an
  interrupt on the parent turn cannot reach it; only `SubagentKill` and `shutdown_all` can. The
  `MAX_CONCURRENT_SUBAGENTS` permit is acquired *inside* the spawned task, not before detaching, so a
  full pool delays a background run instead of blocking the turn that started it. `run_child` returns
  an explicitly boxed `Send` future because `run_delegation → detach → run_child → core_loop →
  run_delegation` is a cycle that `Send` inference cannot close on its own.
- **Every background run wakes the agent when it finishes — `watch` only adds wakes for output
  while a bash run is still going.** Waiting is therefore never a reason to set `watch`, and the tool
  descriptions send a waiting agent to end its turn rather than re-read `BashOutput`; that is the
  whole anti-polling design, so do not reintroduce a blocking read or a wait timeout. A wake is
  suppressed exactly when the agent already knows: it read the exit through `BashOutput`, or it
  stopped the run itself with `BashKill` / `SubagentKill`. `BashOutput` cannot be dropped in favour of
  the wake: a run that never exits (`pnpm dev`) never fires one, and a `watch` flood auto-clears
  `watched` (`WATCH_FLOOD_LINES`), which would otherwise leave its output unreachable.
- Providers classify wire failures into `StreamError`; the engine decides — retry with jittered
  backoff, reactive compaction on `ContextOverflow`, or abort. Callers never inspect error strings.
- The MCP handshake tries one protocol era and, only when the failure could be the era itself,
  retries once in the other one. `PREFERRED` picks which era goes first — legacy (`2025-11-25`,
  the `initialize` handshake) today, because every reachable server still speaks it, so the retry
  never fires and connecting costs one round trip. `handshake::sort` is the single place that reads
  meaning into an rmcp failure: only `-32022` and `NoCompatibleProtocolVersion` prove a modern peer
  (no retry), transport and auth failures are era-agnostic (no retry), and everything else earns the
  other era. A server's era is never configured — configuring it would be a per-server quirk table.
- The OAuth redirect is **parsed in one place and validated in another**. `goat-auth`'s loopback
  capture returns the whole `AuthorizationResponse` (`code` plus RFC 9207 `iss`) and checks only
  `state`, because that is the one value it issued itself; it has no metadata, so it cannot judge
  the issuer. `goat-mcp` does the discovery, so it passes `iss` to rmcp and lets rmcp compare.
  Do not add issuer validation to `goat-auth`, and do not let a capture helper return just a code —
  dropping the rest of the response is what broke Sentry login.
- Sessions are keyed by `SessionId`, with a secondary index by `conversation_id`. There is no cwd map:
  `goat code` defaults to a **new** session, and only `-c` resolves cwd to the latest conversation through
  a database query. Several live sessions can share a cwd.
- `goat daemon serve` takes `~/.goat/daemon.lock` first, builds one `CodeSessionHub`, **binds the socket
  and starts accepting**, and only then opens `ProxyStore`, spawns a `Recorder` with two `Meter`s
  (one code, one agent), boots the agent runtime via `AgentRuntime::boot_with_code_metered`, backfills
  `rate_limits.json` and serves the proxy dashboard — all under one shutdown token. Binding before
  the boot is deliberate: nothing that touches the network may come before the socket, or every
  client wait budget is a lie. `daemon.status` reports `ready: false` until the agent runtime is up;
  code sessions never wait on it. The `code_task` tool drives that same `CodeSessionHub` in-process,
  no wire hop.
- **The daemon greets first with its method table, and nobody gates a version.** On accept it sends
  one `Hello` carrying every `(method, versions)` its router holds, its grant set, and an `info`
  object (`build`, `epoch`, `pid`, `client_id`); `Api::negotiated` picks a version per method from
  it. Skew is therefore per method and typed — calling something the daemon does not serve fails
  with `ErrorCode::UnsupportedVersion` before the call leaves the client — instead of one
  connection-wide compatible/incompatible verdict. `build` rides in `info` rather than as a `Hello`
  field so `envelope_fingerprint` does not move every time a deployment fact is added; it decides
  nothing and only feeds a human-readable line. Destructive judgment belongs to the state's owner:
  `admin.daemon_stop { if_idle }` lets the daemon refuse while a turn is running, which a client
  reading a connect-time snapshot could not do without racing a `cron` turn. `goat daemon stop`
  waits for EOF, which the daemon sends only after it has fully drained.
- **The lock, not the socket, is the handoff barrier.** `transport::cleanup` unlinks the socket
  while the runtime still has up to 10 s of drain left, so "socket gone" does not mean "process
  gone". An `flock` is released by the kernel on process death, SIGKILL included, so a replacement
  daemon blocks on `goat_daemon::acquire` until the old one is really out. That is also what makes
  the global boot sweeps (`WHERE status = 'running'`) safe.
- **Detaching is a spawn contract, not a guess.** `goat daemon serve` stays in the foreground so a
  supervisor can own it; `goat daemon start` spawns it with the hidden `--detached`, and only then
  does it call `rustix::process::setsid()`. Deciding from job control instead would be wrong in a
  non-interactive shell, where the process is not a group leader and would detach from its
  supervisor.
- `goat integration` manages the global connection to a service; `goat agent integration` binds an
  already-connected service to one agent. Both take `-a <agent>` where an agent is implied.
- `interact::pick` is the one non-cancellable picker: it promotes Esc to `ConsoleError("cancelled")`,
  while `select_index` and `Table::pick` return `None`.
- **`device` and `remote` are opposite directions, and both are CLI nouns.** `goat device
  {add,ls,rm}` runs on the daemon host and manages who may reach it — `add` mints a one-time pairing
  code (3 min, `goat-remote::pairing`) and prints the server fingerprint. `goat remote
  {add,ls,rm,use}` runs on the client and manages which daemons *this* machine reaches. `local` is a
  reserved remote naming the daemon on this machine; it is always in `goat remote ls`, and
  `goat remote use local` is how you turn remote off — there is no separate enable flag. The
  matching config keys are `devices` (bind/advertised, server side; still accepts the old `remote`
  key via serde alias) and `remotes` + `default_remote` (client side). `default_remote: None` means
  `local`, so local has exactly one representation.
- **The server never proves its name, only its key.** `ca.rs` issues a SAN-less leaf when
  `advertised` is empty and skips regeneration once `server.crt` exists, so the client pins the
  server fingerprint (`verify::PinnedServer`) instead of validating a hostname. Addresses can then
  be bare IPs, Tailscale names, or port-forwards without touching the certificate. Do not add
  hostname validation on top; it would re-introduce the certificate-name problem pinning removes.
- **Device key material is one identity in `credentials.json`**, under
  `{service: remote, provider: <remote name>, account: "device", slot: "key"|"cert"|"ca"}`.
  The three slots are stored together on purpose — a half-restored remote is a broken remote.
  `config.json` keeps only addressing: `host`, `fingerprint`, `last_dir`.
- **A remote client never sends its own cwd.** `session.open` answers with the cwd the daemon
  actually normalized, the client records it as that remote's `last_dir`, and the next
  `goat code --remote <name>` reuses it (`--dir` overrides). Sending the local cwd would open a
  session on a path that does not exist on the daemon host, which `goat-tool`'s `ToolSandbox`
  then fails on for every call. `-w` is refused for remote targets because worktrees are local git.
- `goat-client` is transport-agnostic: `Link::{Local,Remote}` dials either a unix socket or
  mTLS+WebSocket and hands back the same `Api`, so nothing above it knows which it got. Local-daemon
  autostart lives behind `Link::dial_or_spawn` and fires only for `Link::Local` — a remote target
  that cannot connect must fail, never silently start a second daemon here.
- **Two config files, split by who reads them.** `~/.goat/config.json` is the daemon's — `search`,
  `web_fetch`, `proxy`, `integrations`, `providers`, `devices`, all read inside the daemon.
  `~/.goat/client.json` is the client's — `theme`, `mouse_capture_enabled`, `remotes`,
  `default_remote`, read only by the TUI and the CLI dialling out. One file with two owners is why
  "who writes this" had no principled answer. A first load with no `client.json` adopts those four
  keys out of `config.json` **and removes them from it**, so a key never lives in two places; the
  adoption takes only the keys the client owns, so the daemon's stay behind.
- **The daemon's config has one writer: `admin.config_edit`.** It takes a list of `ConfigEdit` —
  a closed set of intents (`provider_set`, `search_default_set`, `integration_remove`, …) — and the
  daemon is the only place that opens the file. One method rather than nine keeps the table small;
  the enum is the vocabulary. Provider-shaped payloads (a search account, an integration entry) cross
  as opaque JSON so `goat-api` never learns a concrete provider name. `goat provider`,
  `goat search` and `goat agent integration` call it, and a local target autostarts the daemon so a
  write still works from a cold machine.
- **Credentials cross the same door, and `Attachment` carries it.** `admin.credential_set` /
  `admin.credential_remove` take a `goat_auth::CredentialKey` plus a `CredentialValue` — the same
  `#[serde(tag = "kind")]` encoding `credentials.json` uses on disk, reused rather than mirrored so
  the two cannot drift. `Credential` itself stays unserializable; that is the guardrail keeping a
  secret out of a log line. **Acquisition is the client's, storage is the daemon's**: OAuth
  loopback, prompts and device flows run where the human is, and only the acquired credential
  crosses. A write fans `Op::RefreshAccounts {}` out to every live session, which is why a
  `goat provider login` in one terminal reaches a running `goat code` in another. Storing keeps a
  credential that failed validation and reports `verification_failed` — a network blip must not
  delete a key.
- **A slash command reaches the daemon through `CommandEffect::Admin(Vec<AdminRequest>)`,** a
  batch so `/search` can write a key and its config rows in one order. `Attachment` carries
  `admin: Sender<AdminRequest>` beside its `Op` sender, and `goat-client`'s pump is where an
  `AdminRequest::ProviderLogin` turns into a local OAuth run plus one `admin.credential_set`. A
  remote link refuses `ProviderLogin` outright — a remote daemon reads its own credentials.
  `goat-tui` and `goat-command-*` name credential keys but never construct a `CredentialStore`, so
  a client-side write is unrepresentable rather than merely absent.
- **`goat mcp` is the one direct writer left, and deliberately so.** Its server secrets and
  `mcp.json` move as a pair with rollback, and `ConfigEdit` has no MCP vocabulary — splitting the
  pair across a process boundary would buy nothing and lose the rollback. Device key material in
  `goat remote` is not an exception at all: the client reads it, so the client owns it.

## Vestigial — present in code, does nothing

Do not build on these, and do not describe them as features:

- Per-agent `EmbeddingSettings` — collected per agent, then `boot_inner` takes
  `embedders.values().next()` for the single global `MemoryEngine`, so with more than one configured
  agent the winner is arbitrary. Only `openai` is implemented; other values warn and are skipped.
  This one is a design bug rather than dead code: the fix is to decide whether embedding is global
  or per-agent, not to delete a field.

Everything else this section used to list has been removed rather than documented —
`AutonomyConfig`, `MemoryConfig.episodic_k`, goal review (`goals_due_for_review`,
`idx_goals_review`, `goals.parent`), `set_paused`/`is_paused` and its three gates, `core_memory`,
`Event::ProcessObserved`, `Op::Login`, `GoatPaths::agent_dir`, `GoatPaths::memory_dir`, and
`Model::with_account`. Schema removals ride `0027_drop_vestigial.sql`; migration files are never
deleted, since `sqlx::migrate!` checksums the ones already applied. If you find yourself adding an
entry here, delete the thing instead.

There is no `self-tick` and no goal-review scheduling. Both were removed in `7c2a7ad`; the only
schedule kinds are `once` and `cron`, and an agent gets them only by calling the `schedule` tool.
Two `goat-brain` test names still say `self_tick` — they exercise `TurnMode::Schedule`.

## Testing

The full-screen TUI needs a real tty, so it is not driven headlessly. Test the pure `App::update`
reducer and the engine's `Op → Event` behavior instead. `App::new` takes an `Origin`, so a remote
session is testable without a daemon — that is how the header's local git/gh chrome is proven off
for remote targets. Non-TUI binary paths (`--version`, `--help`, `update`, `--print-log-path`) are
safe anywhere. The headless bridge needs no tty; its codec round-trips and shutdown handshake are
unit-tested in `goat-code`'s `headless` module. `goat-daemon`'s `remote_e2e` drives the real
`goat_remote::client` rather than a hand-rolled TLS helper, so the client half is covered by the
same test that covers the server half.

Tests are inline `#[cfg(test)]` modules by default, concentrated in `goat-tui` and `goat-engine`.
The only `tests/` directories are `goat-daemon`, `goat-remote`, and `goat-tool-browser` (real
Chrome).

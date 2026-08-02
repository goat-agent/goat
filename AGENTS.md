# AGENTS.md — goat

goat is a single-user, single-host personal AI product in Rust with two capabilities:

- **agent** — an autonomous actor holding a resident chat connection (Discord gateway, Slack Socket
  Mode). It reacts to
  messages, runs `once`/`cron` tasks it registers for itself through the `schedule` tool,
  consolidates memory nightly at 04:00, and delegates coding to the code engine in-process.
- **code** — a terminal coding agent rendered as a full-screen TUI, always spoken to through the
  resident daemon.

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

Use `cargo nextest`, **not** `cargo test`. `.config/nextest.toml` pins `goat-daemon` tests to
`max-threads = 1`; plain `cargo test` ignores that file and races them.

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
  append variants and keep wildcard handling. `goat-protocol::Event` and `goat-provider::StreamChunk`
  are not — changing them breaks the whole wire and provider surface at once.
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
    `goat-tool-browser` and `goat-tool-computer` bypass it and are wired directly into
    `GoatAgent::new` behind `config.browser_enabled` / `config.computer_use_enabled`.
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
  `Residue::Keep` leaves (github, sentry, slack, langfuse) forward unrecognized tokens verbatim to
  the service's native search language — including bare terms, so their `TermPolicy` never fires;
  `Residue::Reject` leaves (linear, notion, tiro) hard-error on unknown keys, and only there does
  `TermPolicy::Reject` refuse free text. `limit:` is resolver-reserved, `@me` is the one
  self-reference, and stream names key persisted `WatchState`, so default stream names never change.
- Connections are global; `IntegrationAuth` decides how one is established — a pasted `Secret`, an
  `OAuth` round trip, or `External`, meaning a host tool such as `gh` owns the credential and the
  `config.json` entry is itself the connection marker. Per-agent binding lives in the agent's
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

`crates/` is flat, 101 crates, every one prefixed `goat-`. The prefix tells you the family:
`goat-agent*` is the autonomous actor, `goat-code`/`goat-core`/`goat-engine`/`goat-tui` and the
`goat-tool-*`/`goat-command-*` families are coding, and `goat-provider*`/`goat-store`/`goat-config`/
`goat-auth`/`goat-console`/`goat-protocol`/`goat-proxy` are shared. `ls crates/` beats any list
kept here.

Placements that contradict the naming:

- `goat-skill` (singular) is a code crate; `goat-skills` (plural) is an agent crate. Different
  scopes, different parsers.
- `goat-provider-openai-compat` registers nothing — it is the chat/Responses wire base.
  `goat-provider-builtin` is the product's provider table built over it: one `Row` per data-only
  provider, covering thirteen hosted providers plus the local trio (ollama, lmstudio, llama-cpp).
- `goat-integration-mcp` registers nothing either — same idea, one family over. It is the shared base
  every hosted-MCP integration builds on, so a leaf is a `McpService` descriptor plus its parser.
- `goat-mcp` is the **protocol** crate: transports (stdio and streamable HTTP), session lifecycle,
  result extraction, error classification, OAuth. It knows neither tool system — `goat-engine` owns
  the `goat_tool::Tool` adapter for local stdio servers, `goat-integration-mcp` owns the
  `goat_agent_tool::ToolHandler` passthrough for hosted ones. Do not put a tool adapter in it.
  Its `handshake` module is the only place that names a protocol revision or decides which one to
  speak; no other crate mentions an MCP version.
- `goat-integration`'s `watch` module is the one polling driver (`run_workflow`), and it does not
  know rmcp — that is why `goat-integration-github`, which shells out to `gh`, uses it too. Diff
  state stays per source under the unchanged `(agent, integration, account, stream)` key even
  inside a multi-source workflow. `diff::{REBUILD, RETAIN, SETTLE}` are the three re-fire policies;
  they encode opposite intents on purpose, so pick one rather than unifying them.
- `goat-embedding` is agent-side and reaches memory only through `goat-runtime`'s adapter;
  `goat-memory` defines its own `Embedder` trait and does not depend on it.
- `goat-command-*` is the **TUI** slash-command family. Channel slash commands are
  `goat-agent-command-*`.
- `Manager` lives in `goat-daemon`, not in an engine crate.

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
  to report, not a removal to act on. The trigger is a local-only `ClientFrame::ReloadAgents`, and
  every CLI that writes config calls it after writing, so nothing tells the user to restart the
  daemon any more. Only a new binary still needs one.
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
- Sessions are keyed by `SessionId`, with a secondary index by `thread_id`. There is no cwd map:
  `goat code` defaults to a **new** session, and only `-c` resolves cwd to the latest thread through
  a database query. Several live sessions can share a cwd.
- `goat daemon serve` takes `~/.goat/daemon.lock` first, builds one `Manager`, **binds the socket
  and starts accepting**, and only then opens `ProxyStore`, spawns a `Recorder` with two `Meter`s
  (one code, one agent), boots the agent runtime via `Goat::boot_with_code_metered`, backfills
  `rate_limits.json` and serves the proxy dashboard — all under one shutdown token. Binding before
  the boot is deliberate: nothing that touches the network may come before the socket, or every
  client wait budget is a lie. `Welcome.ready` is false until the agent runtime is up; code sessions
  never wait on it. The `code_task` tool drives that same `Manager` in-process, no wire hop.
- **The daemon greets first and never judges a client.** On accept it sends `ServerFrame::Welcome`
  carrying the wire fingerprint, the exe identity, and a `Busy` snapshot; there is no `Hello` and no
  version gate. The client decides (`goat_client::decide`): a different wire means they cannot talk,
  a different exe means the daemon runs older logic, and `Busy` decides whether replacing it is
  allowed. `StopDaemon` is never gated by any of that — it is the escape hatch. `goat daemon stop`
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

## Vestigial — present in code, does nothing

Do not build on these, and do not describe them as features:

- `goat-plugin` — no `impl Plugin` anywhere, no `inventory::iter::<PluginFactory>()`, and
  `goat-runtime` declares the dependency without importing it.
- `AutonomyConfig.enabled` — parsed from `config.json` under `deny_unknown_fields`, then read by
  nothing. Setting it has no effect.
- `MemoryConfig.episodic_k` — parsed and stored, never passed to `BrainDeps`; recall hardcodes 6.
- Per-agent `EmbeddingSettings` — collected per agent, then `boot_inner` takes
  `embedders.values().next()` for the single global `MemoryEngine`, so with more than one configured
  agent the winner is arbitrary. Only `openai` is implemented; other values warn and are skipped.
- Goal review — `next_review_at`, `goals_due_for_review`, and `idx_goals_review` are complete and
  unit-tested, but nothing outside `goat-store` calls them. There is no trigger. `goals.parent` is
  likewise always `None`; the tool schema has no parameter for it.
- `set_paused` has no callers, so the `is_paused` gate in the scheduler, runtime, and integrations
  can never be closed.
- `core_memory` survives `0012_drop_v1_memory.sql` awaiting a `goat memory migrate` command that
  does not exist.
- `Event::ProcessObserved` is consumed by the TUI but published by nobody; `Op::Login` has a full
  engine handler but no client constructs it. `GoatPaths::{agent_dir, memory_dir}` and
  `Model::with_account` have zero callers.

There is no `self-tick` and no goal-review scheduling. Both were removed in `7c2a7ad`; the only
schedule kinds are `once` and `cron`, and an agent gets them only by calling the `schedule` tool.
Two `goat-brain` test names still say `self_tick` — they exercise `TurnMode::Schedule`.

## Testing

The full-screen TUI needs a real tty, so it is not driven headlessly. Test the pure `App::update`
reducer and the engine's `Op → Event` behavior instead. Non-TUI binary paths (`--version`,
`--help`, `update`, `--print-log-path`) are safe anywhere. The headless bridge needs no tty; its
codec round-trips and shutdown handshake are unit-tested in `goat-code`'s `headless` module.

Tests are inline `#[cfg(test)]` modules by default, concentrated in `goat-tui` and `goat-engine`.
The only `tests/` directories are `goat-daemon`, `goat-remote`, and `goat-tool-browser` (real
Chrome).

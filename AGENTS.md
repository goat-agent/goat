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
- `ProfileId` is explicit, constructor-injected, never ambient. `ProfileId::from_slug` must stay
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

- Providers live in `goat-provider-<name>`, channels in `goat-channel-<name>`, integrations in
  `goat-integration-<name>`, search backends in `goat-search-provider-<name>`. Shared crates must
  never know a concrete name (`openai`, `discord`, …).
- Provider-specific request bodies, streaming, auth, and error mapping stay inside each provider
  crate. No shared provider "quirks" flags.
- **Registration is not uniform — check before assuming `inventory` picks your crate up:**
  - channels and integrations: `inventory` + `pub const ID` via `from_static(...)`. A channel's
    `ChannelFactory` also carries `metadata: fn() -> ChannelMetadata`, which is how it declares its
    display name, its setup text, and one `SecretSpec` per secret it needs — the CLI drives its
    prompts off that list, so a channel that forgets it gets asked for nothing.
  - agent commands: `inventory`, but the constant is a plain `pub const ID: &str`.
  - agent tools: `inventory` + `pub const NAME: ToolName` for `fs`/`shell`/`skill` only. `goal`,
    `memory`, `pty`, `code`, and `schedule` need injected runtime deps and are wired by explicit
    `register()` calls in `goat-runtime`.
  - LLM providers and search providers: **no `inventory` at all.** `Registry::load_metered` and
    `goat-search-providers::metadata` build hardcoded lists, and identity is a runtime
    `ProviderId::from("…")`. Adding a provider means editing that list by hand.
  - code tools: `ToolRegistry::builtin()` aggregates fs, shell, search, skill, and web.
    `goat-tool-browser` and `goat-tool-computer` bypass it and are wired directly into
    `GoatAgent::new` behind `config.browser_enabled` / `config.computer_use_enabled`.
- **A channel owes no tools.** It is a presence, not a reach: it holds a resident connection under a
  bot identity and turns inbound traffic into `IncomingMessage`. Workspace-wide search and posting
  where the bot is not a member belong to the matching integration. `slack` is deliberately both —
  `goat-channel-slack` is the bot people address (`xoxb-` + `xapp-`, Socket Mode) and
  `goat-integration-slack` reaches in as the owner (`xoxp-`, hosted MCP). Their token capabilities
  are disjoint, so the two cannot be merged and neither is redundant.
- **An integration owes neither tools nor a watcher.** Tools are usually discovered from a hosted
  MCP server's `list_tools`, and a watcher polls and publishes `Event::IntegrationUpdate` on
  deterministic diffs — but a connection plus a watcher is already a complete integration
  (`goat-integration-github` registers no tools; the agent reaches GitHub through `shell` and `gh`),
  and so is a connection plus tools (`goat-integration-posthog` has no watcher). Watchers own
  polling and diffing, never the policy of what is worth watching — that is declared per-agent in
  the binding config, and several watchers decline to spawn until it is.
- Connections are global; `IntegrationAuth` decides how one is established — a pasted `Secret`, an
  `OAuth` round trip, or `External`, meaning a host tool such as `gh` owns the credential and the
  `config.json` entry is itself the connection marker. Per-agent binding lives in the agent's
  `integrations` config map. Raw observations persist losslessly in `integration_observations`.
- Channel bindings are per-agent, and **no secret ever lives in `config.json`.** The `channels.<kind>`
  map records *that* an agent uses a channel — an empty object is a complete binding, so never delete
  one for looking empty — while every secret sits in `credentials.json` under
  `{ service: channel, provider: <kind>, account: <agent slug>, slot: <secret name> }`. `slot` is the
  axis that lets one binding hold several secrets; `account` is the agent, not a workspace. A boot
  that finds a declared slot sitting in `config.json` moves it into the store and rewrites the file
  (`goat-runtime::channel_secrets`); the stored value always wins over a stale config one.

## Where things live

`crates/` is flat, 94 crates, every one prefixed `goat-`. The prefix tells you the family:
`goat-agent*` is the autonomous actor, `goat-code`/`goat-core`/`goat-engine`/`goat-tui` and the
`goat-tool-*`/`goat-command-*` families are coding, and `goat-provider*`/`goat-store`/`goat-config`/
`goat-auth`/`goat-console`/`goat-protocol`/`goat-proxy` are shared. `ls crates/` beats any list
kept here.

Placements that contradict the naming:

- `goat-skill` (singular) is a code crate; `goat-skills` (plural) is an agent crate. Different
  scopes, different parsers.
- `goat-provider-openai-compat` registers nothing — it is the shared base thirteen OpenAI-compatible
  crates build on. `goat-provider-local` registers three providers (ollama, lmstudio, llama.cpp).
- `goat-embedding` is agent-side and reaches memory only through `goat-runtime`'s adapter;
  `goat-memory` defines its own `Embedder` trait and does not depend on it.
- `goat-command-*` is the **TUI** slash-command family. Channel slash commands are
  `goat-agent-command-*`.
- `Manager` lives in `goat-daemon`, not in an engine crate.

## Filesystem

Everything is under `~/.goat/`, laid out by `goat-config`'s `GoatPaths`; `HOME` is the only thing
that moves it. Read `crates/goat-config/src/paths.rs` for the full list. The parts that mislead:

- **Memory and skills are not per-agent.** `agents/<slug>/` holds only `agent.md` and `config.json`.
  Memory is one global tree at `memory/<scope>/` keyed by `Scope` (`owner`, `self`, `domain/<name>`);
  per-persona skills live at `profiles/<slug>/skills/`.
- `~/.goat/agents/*.md` does double duty: `AgentRegistry::load` also scans it (plus the project-local
  `.goat/agents/`) as the **code engine's subagent registry**.
- `~/.agents/skills` is a third, separate skill scope. `<repo>/.goat/worktrees/` is created inside
  the *target* repository and is unrelated to the home tree.
- `goat.db` is one file holding three table sets: unprefixed agent tables, `code_`, and `proxy_`.

## Non-obvious behavior

- `Engine`'s only method takes `self` by value, so it is not callable through a trait object;
  nothing uses `dyn Engine`. Decoupling comes from generics plus bounded `tokio::mpsc` channels
  (32 ops, 512 events) carrying `goat-protocol`. The trait avoids `async_trait` and `Stream` —
  both are used freely elsewhere.
- `code_messages` is insert-only; compactions live in `code_compactions`. `/resume` rebuilds engine
  history from the **latest compaction alone**, while the transcript replays full scrollback with a
  marker per compaction.
- Providers classify wire failures into `StreamError`; the engine decides — retry with jittered
  backoff, reactive compaction on `ContextOverflow`, or abort. Callers never inspect error strings.
- Sessions are keyed by `SessionId`, with a secondary index by `thread_id`. There is no cwd map:
  `goat code` defaults to a **new** session, and only `-c` resolves cwd to the latest thread through
  a database query. Several live sessions can share a cwd.
- `goat daemon serve` builds one `Manager`, opens `ProxyStore`, spawns a `Recorder` with two
  `Meter`s (one code, one agent), boots the agent runtime via `Goat::boot_with_code_metered`,
  backfills `rate_limits.json`, serves the proxy dashboard, and runs `serve_with` — all under one
  shutdown token. The `code_task` tool drives that same `Manager` in-process, no wire hop.
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
- Per-agent `EmbeddingSettings` — collected per profile, then `boot_inner` takes
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

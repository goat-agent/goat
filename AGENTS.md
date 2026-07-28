# AGENTS.md — goat

goat is a single-user, single-host personal AI product in Rust with two capabilities:

- **agent** — an autonomous actor resident on chat channels (Discord). It reacts to
  messages, owns self-tick and goal-review scheduling, keeps lifetime memory, consolidates
  nightly, and delegates coding to the code engine in-process.
- **code** — a terminal coding agent rendered as a full-screen TUI, backed by the resident daemon,
  holding live sessions keyed by cwd.

One binary (`goat`), one daemon, one database (`~/.goat/goat.db`), one config tree (`~/.goat/`).
This file is the source of truth for agents working here; `CLAUDE.md` imports it. When a crate
grows its own conventions, add a nested `crates/<name>/AGENTS.md` — the closest file wins.

## Commands

| Command | Purpose |
|---------|---------|
| `cargo build --workspace` | Build every crate |
| `cargo test --workspace` | Run all tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint; warnings are errors |
| `cargo fmt --all` | Format (`--check` to verify only) |

`cargo fmt --all`, the `clippy` line, and `cargo test --workspace` must all pass before any change
is done. For a narrow change, run the smallest relevant check; for a broad one, run all four.

## CLI

Three groups, by ownership: bare `goat` is shared, `goat code` is coding, `goat agent` is the
autonomous actor. Bare `goat` prints help.

```
goat setup | doctor | provider | update | daemon | remote   shared
goat integration add | list | remove                         external-service connections
goat code [-c] [-w <name>] [--headless] [-p]                 coding TUI
goat code worktree | search                                  coding subcommands
goat agent list | add | show | remove                        agent management
goat agent channel | status | log                            agent channels and state
```

`goat daemon serve` runs both subsystems in one process; the first client that needs it auto-spawns
it (`connect_or_spawn`) and it stays resident, so agents run without a separate `run` command. There
is no service installation and no ambient "current agent" — with more than one agent, `-a <agent>`
is required.

## Filesystem

```
~/.goat/
├─ credentials.json          provider keys (shared goat-sdk format)
├─ config.json               product settings
├─ goat.db                   the one database (agent tables + code_ tables)
├─ bin/goat
├─ memory/                   long-term memory (files + FTS5 + sqlite-vec index)
├─ skills/
├─ agents/<slug>/            agent.md, config.json, memory/, skills/
├─ remote/                   paired devices, CA
├─ browser/  logs/  update/
```

## Workspace

`crates/` is flat; every crate is prefixed `goat-`. Four families.

### Shared

The inlined provider SDK and the terminal design system — used by both capabilities.

- `goat-protocol` — wire contract (`Op`, `Event`, `TaskId`); serde only; leaf.
- `goat-provider` / `goat-provider-*` / `goat-providers` — the `Provider` trait, one crate per LLM
  provider, and the registry. Providers classify their own wire errors; callers never inspect
  error strings.
- `goat-auth` — credential store (API keys, OAuth tokens).
- `goat-console` — the terminal design system (see below).
- `goat-store` — the one SQLx/SQLite store: agent tables plus `CodeStore` over the `code_`-prefixed
  coding tables, one migration history, FTS5 + sqlite-vec.
- `goat-config` — owns the `~/.goat/` layout, product settings, and agent definitions.
- `goat-sqlite-vec` — FFI isolation for the statically linked `sqlite-vec` extension.
- `goat-proxy` — provider usage metering (`MeteredProvider` + `Recorder`, wired through
  `Registry::load_metered`), the daemon-hosted localhost dashboard (usage, rate limits, request
  log) with account management (API key + OAuth login, backfilled from `rate_limits.json`);
  state lives in the `proxy_`-prefixed tables via `goat-store`'s `ProxyStore`.

### agent

- `goat-agent` — library exposing the `goat agent`/`setup`/`doctor` CLI (`cli` module).
- `goat-types`, `goat-bus`, `goat-model` — IDs/events, event bus, model registry.
- `goat-channel` / `goat-channel-*` — channel trait and one crate per chat channel.
- `goat-integration` / `goat-integration-*` — external-service integrations, one crate per vendor.
  An integration contributes tools (discovered from the service, e.g. a hosted MCP server's
  `list_tools`) and an optional polling watcher that publishes `Event::IntegrationUpdate` on
  deterministic diffs. Connections (OAuth/keys) are global; per-agent binding lives in the agent's
  `integrations` config map. Raw observations persist losslessly in `integration_observations`.
- `goat-brain` — per-agent conversation loop and turn handling.
- `goat-runtime` — wires the runtime over trait registries; `Goat::boot`/`boot_with_code`.
- `goat-profile` — agent config value objects.
- `goat-memory` — lifetime memory (files, `facts`, derived FTS5 + sqlite-vec) behind `MemoryEngine`.
- `goat-sleep`, `goat-loop`, `goat-skills`, `goat-render`, `goat-plugin`.
- `goat-agent-tool`, `goat-agent-tool-*` — the agent tool trait and one crate per tool
  (fs, shell, skill, schedule, goal, memory, pty, code). `goat-agent-tool-code` delegates a coding
  task to the code engine in-process via `Manager::delegate_code`.
- `goat-agent-command`, `goat-agent-command-skill` — channel slash commands.

### code

- `goat-code` — the `goat` binary: CLI, logging, and the unified `goat daemon`.
- `goat-core` — `Session` and the `Engine` trait; owns the `Op → Event` loop and nothing else.
- `goat-engine` — `GoatAgent`, the production `Engine`: LLM loop, tool dispatch, retry, mid-turn
  steering, auto-compaction, and the `Agent` delegation tool. See `crates/goat-engine/AGENTS.md`.
- `goat-tui` — full-screen ratatui app (The Elm Architecture).
- `goat-wire`, `goat-daemon`, `goat-client`, `goat-remote` — the daemon wire contract, the resident
  daemon (`Manager`), the thin client, and mTLS-over-WebSocket remote access.
- `goat-worktree`, `goat-sandbox`, `goat-mcp`.
- `goat-tool`, `goat-tool-*`, `goat-tools` — the code tool trait, one crate per tool, and registry.
- `goat-command`, `goat-command-*`, `goat-commands` — the TUI command trait, per-category crates,
  and registry.
- `goat-skill`, `goat-github`.

## Design system

`goat-console` is the shared terminal design system; every CLI surface renders through it. It is
domain-free — it knows tokens, structure, and interaction, never providers, accounts, or agents.

- `color` — `ColorMode`, `Palette`, and the `cell`/`paint` text helpers: the tokens.
- `layout` — `Table`, `Footer`, `cell`/`section`/`line`, and `pair`/`pair_styled`: structure.
  `Table` is the one list component — the same styled rows both `render` to output and `pick` an
  index interactively, so a list and its selectable form never diverge. `pair` is the one
  label/value row (`pair_styled` colours the value); there is no second row printer.
- `interact` — `select_index`/`pick` (index/value pickers), `prompt`/`secret` (text and hidden
  input), `confirm`, and status lines (`success`/`warning`/`note`). One text primitive and one
  secret primitive, both over plain labels. All are cancellable: pickers return `None` on Esc,
  and `prompt`/`secret` return `None` on an empty submit (surfaced as an `❯`-marked prompt with a
  muted "go back" hint), so every step of a flow can back out to the previous one.
- `theme` — the single dialoguer `Theme` (`❯` accent marker, muted defaults, warning-coloured
  errors) shared by every picker and prompt.
- `error` — `ConsoleError` plus `report`/`fail`: one failure format.

It is error-agnostic: `cell` is generic over the closure error, and interactive helpers return
`ConsoleError` that both `anyhow` and `color_eyre` absorb through `?`. Domain UI (provider/account
resolution, auth-method choice) lives in each binary's `ui` facade, which also re-exports the system
and supplies its own `report`/`fail` in its native error type, so the UI error never leaks across the
boundary. Do not add a second styling or prompt system, and do not push domain concepts into
`goat-console`.

## Rules

- **No comments.** None of any kind — no `//`, `///`, `//!`, block comments, or TOML `#`. Convey
  intent through names and structure.
- Centralize dependency versions in the root `[workspace.dependencies]`; crates inherit with
  `{ workspace = true }`.
- `unsafe` is forbidden workspace-wide (`unsafe_code = "forbid"`). The only opt-outs are the two
  FFI-isolation leaves, `goat-sqlite-vec` and `goat-agent-tool-pty`. Do not add a third; isolate
  new FFI in its own leaf crate.
- Edition 2024, MSRV 1.95. clippy `pedantic` at warn; keep the tree clean under `-D warnings`.
- Errors: library crates use `thiserror` enums. The agent binary/runtime boundary uses `anyhow`;
  the code application boundary uses `color_eyre::Result`. The shared `goat-console` returns
  `ConsoleError`, converted at each boundary's `ui` facade.
- **Logging goes to a rolling file, never stdout/stderr** — stdout corrupts the full-screen TUI.
  Use `tracing`; `GOAT_LOG` sets the filter.
- `ProfileId` is explicit, constructor-injected, never ambient. `ProfileId::from_slug` must stay
  deterministic; the `GOAT_NAMESPACE` UUID value must never change — it keys every stored id.
- `Event` and `LlmChunk` are `#[non_exhaustive]`; append variants and keep wildcard handling.
- Do not skip hooks with `--no-verify`. Do not force-push.

## Extension boundaries

- Providers live in `goat-provider-<name>`; channels in `goat-channel-<name>`; integrations in
  `goat-integration-<name>`.
- Shared crates must not know concrete provider/channel names (`openai`, `discord`, …).
- Concrete extension crates are linked by the final binary; runtime discovers them through
  inventory registries. Provider/channel crates expose `pub const ID` via `from_static(...)`.
- Provider-specific request bodies, streaming, auth, and error mapping stay inside each provider
  crate. No shared provider "quirks" flags.

## Architecture

- `Engine` is an object-safe actor: `fn spawn(self, ops, events) -> JoinHandle`. No `async_trait`,
  no `Stream`. The UI and engine communicate only through `goat-protocol` over bounded
  `tokio::mpsc` channels.
- `GoatAgent` owns a `Conversation` (single source of truth for the LLM context); the TUI keeps an
  append-only render mirror from `Event`s. Messages persist losslessly as `Vec<ContentBlock>` JSON;
  the table is append-only. Compactions are recorded separately, so `/resume` rebuilds the
  compacted engine history while the transcript replays full scrollback with markers.
- Long-running policy is split by ownership: providers classify wire failures into `StreamError`;
  the engine decides — retry with jittered backoff, reactive compaction on overflow, or abort.
- `goat daemon serve` builds one code `Manager`, boots the agent runtime with it
  (`Goat::boot_with_code`), and runs the socket server (`serve_with`) under one shutdown token. The
  `code_task` tool drives that same `Manager` in-process — no wire hop.
- Memory is currently assistant-global by `Scope` (`owner`/`self`/`domain`), not per-agent.

## Testing

The full-screen TUI needs a real tty, so it is not driven headlessly. Test the pure `App::update`
reducer and the engine's `Op → Event` behavior instead. Non-TUI binary paths (`--version`,
`--help`, `update`, `--print-log-path`) are safe anywhere. The headless bridge needs no tty; its
codec round-trips and shutdown handshake are unit-tested.

# AGENTS.md — goat

goat is a single-user, single-host personal AI product in Rust with two capabilities:

- **agent** — an autonomous actor holding a resident Discord gateway connection. It reacts to
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
  - channels and integrations: `inventory` + `pub const ID` via `from_static(...)`.
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

## Where things live

`crates/` is flat, 90 crates, every one prefixed `goat-`. The prefix tells you the family:
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

## Testing

The full-screen TUI needs a real tty, so it is not driven headlessly. Test the pure `App::update`
reducer and the engine's `Op → Event` behavior instead. Non-TUI binary paths (`--version`,
`--help`, `update`, `--print-log-path`) are safe anywhere. The headless bridge needs no tty; its
codec round-trips and shutdown handshake are unit-tested in `goat-code`'s `headless` module.

Tests are inline `#[cfg(test)]` modules by default, concentrated in `goat-tui` and `goat-engine`.
The only `tests/` directories are `goat-daemon`, `goat-remote`, and `goat-tool-browser` (real
Chrome).

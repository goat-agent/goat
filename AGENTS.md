# AGENTS.md — goat

A single-user, single-host personal AI product in Rust. One binary (`goat`), one daemon, one database
(`~/.goat/goat.db`), one config tree (`~/.goat/`).

Two capabilities share them:

| | What it is |
|---|---|
| **agent** | An autonomous actor holding a resident chat connection (Discord gateway, Slack Socket Mode). It reacts to messages, runs `once`/`cron` tasks it registers for itself through the `schedule` tool, consolidates memory nightly at 04:00, and starts coding tasks in-process through the code engine. Coding-task progress stays in the code conversation. |
| **code** | A terminal coding agent rendered as a full-screen TUI. It always speaks through the resident daemon, which need not be on this machine. |

## Commands

| Command | Purpose |
|---------|---------|
| `cargo build --workspace` | Build every crate |
| `cargo nextest run --workspace` | Run all tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint; warnings are errors |
| `cargo fmt --all` | Format (`--check` to verify only) |

All four must pass before a change is done. Run the smallest relevant check for a narrow change, all
four for a broad one.

Use `cargo nextest`, never `cargo test`. `.config/nextest.toml` pins tests that spawn a real daemon
to `max-threads = 1`, and `cargo test` ignores that file and races them. Add a new socket-binding test
to that filter.

CI adds `--locked`, runs `actionlint`, smoke-tests `goat --version`, and rechecks on the pinned MSRV.

Development builds use optimization level 1 with line tables and normal unwinding. Unoptimized CLI
code exceeds macOS compact-unwind's 24-bit DWARF offset range; light optimization reduces the actual
unwind metadata rather than suppressing the linker warning. Some local variables may be optimized
out while debugging.

## Rules

**No comments.** Not `//`, `///`, `//!`, block comments, or TOML `#`. Carry intent in names and
structure. Intent that needs prose goes in the nearest `AGENTS.md`.

**Never skip hooks with `--no-verify`. Never force-push.** Neither is mechanically enforced.

- Edition 2024, MSRV 1.95. clippy `pedantic` at warn. Keep the tree clean under `-D warnings`.
- Declare versions, `edition` and `rust-version` in the root workspace tables. Inherit with
  `{ workspace = true }`.
- `unsafe` is forbidden workspace-wide (`unsafe_code = "forbid"`). The two FFI-isolation leaves, `goat-sqlite-vec` and
  `goat-agent-tool-pty`, opt out by omitting `[lints.rust]`. Do not add a third; put new FFI in its
  own leaf crate.
- Errors: `thiserror` enums in library crates, `anyhow` at the agent binary/runtime boundary,
  `color_eyre::Result` at the code application boundary (bridged by `goat-code`'s `into_eyre`).
  `goat-console` returns `ConsoleError`, converted at each boundary's `ui` facade.
- **Log to the rolling file, never stdout/stderr**, which corrupts the TUI. Use `tracing`. `GOAT_LOG`
  sets the filter and is the only environment variable the product reads. Deliberate CLI output on
  non-TUI paths goes through `goat-console`.
- `goat-console` is the only styling and prompt system. Keep domain concepts (providers, accounts,
  agents) out of it; domain UI belongs in each binary's `ui` facade. Its one non-cancellable picker is
  `interact::pick`, which promotes Esc to `ConsoleError("cancelled")`; `select_index` and
  `Table::pick` return `None`.
- Pass `AgentId` explicitly through constructors, never ambiently. Keep `AgentId::from_slug`
  deterministic, and never change the `GOAT_NAMESPACE` constant — a fixed `Uuid`, not an environment
  variable — because it keys every stored id.
- Append to `#[non_exhaustive]` types and keep wildcard handling: `goat-types::Event` and the
  channel/command/integration surface types. `goat-protocol::Op`/`Event` and
  `goat-provider::StreamChunk` are **not** `#[non_exhaustive]`, so adding a variant moves every
  method contract or provider carrying it at once. For `Op`/`Event` that surfaces as
  `methods_fingerprint.txt` refusing to match on `session.submit` and `session.watch`.
- Never edit or delete an applied migration; `sqlx::migrate!` checksums them. Express a removal as a
  new migration.
- `goat integration` manages the global connection to a service. `goat agent integration` binds a
  connected service to one agent. Both take `-a <agent>` where an agent is implied.

## Registration is not uniform

Check before assuming `inventory` picks your crate up.

| Kind | How it registers |
|---|---|
| channels, integrations | `inventory` + `pub const ID` via `from_static(...)` |
| agent commands | `inventory`, with a plain `pub const ID: &str` |
| agent tools | `inventory` + `pub const NAME: ToolName` for `fs`/`shell`/`skill` only; the rest are explicit `register()` calls in `goat-runtime` |
| code tools | `ToolRegistry::builtin()`, which `goat-tool-browser` bypasses |
| LLM and search providers | no `inventory` at all; one ordered list in `Registry::load_metered` |

## Crates

`crates/` is flat and every crate is prefixed `goat-`. Run `ls crates/` rather than keeping a list
here. The prefix names the family: `goat-agent*` is the autonomous actor;
`goat-code`/`goat-core`/`goat-engine`/`goat-tui` and the `goat-tool-*`/`goat-command-*` families are
coding; the rest is shared.

Keep concrete leaf names (`openai`, `discord`, …) out of shared crates.

A crate with its own conventions carries its own `AGENTS.md`, and the closest file wins. **Read the
file for the crate you are touching before changing anything below its surface.**

| Crate | Its `AGENTS.md` covers |
|---|---|
| `goat-agent-tool` | the agent-side tool contract, caller and audience |
| `goat-api` | the method surface, its two frozen artifacts, grant-built routers |
| `goat-auth` | credential storage, the acquisition/storage split, the OAuth redirect |
| `goat-capability` | host capability leases, the browser connection, why desktop control left |
| `goat-channel` | why a channel owes no tools; bindings and the secret store |
| `goat-client` | transport-agnostic dialling, autostart, the admin pump |
| `goat-code` | the binary, the headless bridge, remote targets |
| `goat-command` | the TUI slash-command grammar and effects |
| `goat-config` | the two config files and the daemon's single writer |
| `goat-daemon` | boot order, the lock barrier, `Hello`, the subscriber bus, sessions |
| `goat-engine` | module map, backgrounding, detached subagents, compaction |
| `goat-integration` | the watch query DSL, parser skeleton, diff policies |
| `goat-integration-mcp` | the hosted-MCP base every integration leaf builds on |
| `goat-mcp` | protocol eras, the OAuth registration ladder |
| `goat-mcp-tools` | both tool shells over one source trait |
| `goat-memory` | the global scope tree, the embedder direction |
| `goat-provider` | the `Provider` trait and the stream vocabulary |
| `goat-providers` | the registry, its fingerprint, when a provider needs a crate |
| `goat-remote` | both mTLS halves, `device` vs `remote`, fingerprint pinning |
| `goat-runtime` | `goat reload`, respawn semantics, explicit tool registration |
| `goat-sandbox` | the read-only Seatbelt profile around every shell call |
| `goat-skill` | scope layering and the `arguments` grammar |
| `goat-store` | the agent tables, and the four crates owning migrations |
| `goat-tool` | the code-side tool contract and path resolution |
| `goat-tool-browser` | the browser vocabulary over a `Transport` |
| `goat-tui` | slash-command families, testing without a tty |
| `goat-wire` | the frame envelope, framing, the duplex peer |

## Filesystem

Everything sits under `~/.goat/`, laid out by `goat-config`'s `GoatPaths`. `HOME` is the only thing
that moves it. Read `crates/goat-config/src/paths.rs` for the full list. Four parts mislead:

| | |
|---|---|
| **Memory is not per-agent** | `agents/<slug>/` holds `agent.md`, `config.json` and that agent's `skills/`. Memory is one global tree at `memory/<scope>/`. |
| Subagents | `~/.goat/subagents/*.md` plus project-local `.goat/subagents/`, loaded by `SubagentRegistry::load`. |
| Outside the tree | `~/.agents/skills` is a skill scope of its own. `<repo>/.goat/worktrees/` lives in the *target* repository. |
| One db, four owners | `goat-store` and `goat-memory` share the unprefixed namespace; `goat-code-store` owns `code_`; `goat-proxy-store` owns `proxy_`. See `crates/goat-store/AGENTS.md`. |

## Testing

Write inline `#[cfg(test)]` modules. Only `goat-daemon`, `goat-remote` and `goat-code` have `tests/`
directories, because those tests need a real socket.

The TUI needs a real tty, so do not drive it headlessly. Test the pure `App::update` reducer and the
engine's `Op → Event` behavior instead.

No test drives a real Chrome. The browser vocabulary is covered by `goat-tool-browser`'s
`transport::fake`; the extension's end of the pipe is not covered at all.

Two `goat-brain` tests are named `self_tick` but exercise `TurnMode::Schedule`. There is no self-tick.

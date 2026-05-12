# goat

Autonomous personal AI agent in Rust. Single-user, single-host. Runs under persona identities across chat channels, owns self-tick scheduling, evaluates outputs, and stores state under `~/.goat/`.

## Commands

- Format: `cargo fmt --all`
- Build: `cargo build --workspace`
- Test all: `cargo test --workspace`
- Test one crate: `cargo test -p <crate>`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Run daemon: `cargo run`
- Inspect config: `cargo run -- doctor`

Before claiming completion, run the smallest relevant check. For broad refactors, run fmt, clippy, tests, and build.

## Repository layout

- `src/`: binary entrypoint and CLI subcommands.
- `crates/goat-types`: shared IDs, messages, events, wire-neutral types.
- `crates/goat-llm`: provider traits, model IDs, model registry types.
- `crates/goat-llm-*`: one crate per LLM provider.
- `crates/goat-channel`: channel traits and channel registry types.
- `crates/goat-channel-*`: one crate per chat channel.
- `crates/goat-render`: stream rendering.
- `crates/goat-brain`: per-persona conversation loop.
- `crates/goat-runtime`: runtime wiring over trait registries, not concrete extensions.
- `crates/goat-config`, `goat-credentials`, `goat-store`: local config, secrets, persistence.

Keep `crates/` flat. Every crate is prefixed `goat-`.

## Extension boundaries

- LLM providers live in `goat-llm-<provider>`.
- Channels live in `goat-channel-<channel>`.
- Shared crates must not know concrete provider/channel names such as `openai`, `discord`, or `telegram`.
- Concrete extension crates are linked by the final binary in `src/main.rs`.
- Runtime discovers providers/channels through inventory registries.
- Provider/channel crates expose `pub const ID` using `ProviderId::from_static(...)` or `ChannelId::from_static(...)`.
- Use `ProviderId::new(...)` and `ChannelId::new(...)` only for owned runtime/config/input values.

## Design rules

- Provider-specific request bodies, streaming, auth, and error mapping stay inside each `goat-llm-*` crate.
- Do not add shared provider “quirks” flags.
- Channel code should depend on generic `Channel`, `ChannelHandle`, and `ChannelId`, not on other channel crates.
- Channel means goat has its own user-addressable identity. Plugin means external integration without that identity.
- `PersonaId` is explicit, constructor-injected, and never ambient.
- `PersonaId::from_slug` must remain deterministic.
- `Event` and `LlmChunk` are `#[non_exhaustive]`; append variants and keep wildcard handling.
- Library crates use `thiserror`; binary/runtime boundaries may use `anyhow`.

## CLI/UI rules

- Use existing UI helpers under `src/cli/ui.rs`.
- Do not introduce a second table/prompt styling system.
- Keep CLI prompts short and consistent.
- UI-affecting CLI changes need a smoke check with `cargo run -- <subcommand>`.

## Persistence and secrets

- User state lives under `~/.goat/`.
- No XDG split.
- Secrets live only in `~/.goat/credentials.json` or persona channel config files.
- Do not add `.env` or environment-variable fallbacks for secrets unless explicitly requested.

## Guardrails

- No runtime plugin manifest or dynamic plugin loading.
- Plugin/extension selection is Cargo dependency registration plus config presence.
- No `ApprovalQueue` or forced-gate mutation queue.
- No cross-persona memory access without an explicit design decision.
- Do not skip hooks with `--no-verify`.
- Do not force-push.

## Communication

- Be terse and concrete.
- For design work, state the framing before the plan.
- Prefer small, reversible diffs.
- Report changed files and verification evidence.

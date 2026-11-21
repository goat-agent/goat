# AGENTS.md

## Commands

```sh
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p goat-core --test session
cargo test -p goat-core hashline
cargo deny check
```

Run the first three and `cargo deny check` before you finish. The toolchain is pinned in `rust-toolchain.toml`. A `RUSTUP_TOOLCHAIN` variable in the shell overrides the pin; run `cargo +1.100.0 …` in that case.

## Layout

- `crates/goat-core` is the library: agent loop (`session`), providers (`provider/openai`, `provider/anthropic`), the `run` tool and its sandbox (`tool/`), SQLite store (`store`), and accounts with ChatGPT sign-in (`account`). It never prints, reads environment variables or touches the terminal.
- `crates/goat-cli` builds the `goat` binary: the inline input box (`input`), output (`render`), and the commands (`chat`, `login`).
- `docs/style/rust.md` is the binding code style. Read it before writing code.

## Rules

- No comments in code. The only exceptions are listed in `docs/style/rust.md`.
- Keep memory low. Stream instead of buffering, and keep large data on disk. `crates/goat-core/tests/memory.rs` fails on Linux when 50 turns peak above 50 MiB.
- A failed tool call is an `Outcome` with `is_error`, not an `Err`.
- Persist a message only when it is complete. A cancelled turn must leave history that both providers accept.

## Gotchas

- `message::Native` holds provider data that must be replayed byte for byte: OpenAI reasoning items and `phase`, Claude thinking signatures. It is sent back only when the message's model and account equal the current ones. Never edit it.
- Claude rejects edited thinking blocks and any text placed before `tool_result` blocks. History is append-only.
- On the ChatGPT plan, OpenAI requires `store:false` and `stream:true`, and rejects `temperature` and `max_output_tokens`. The Codex login accepts the hosted `web_search` tool (checked live); `webSearch()` uses it in a separate request, so the conversation itself never carries hosted tools.
- The Codex login (`account::codex`) calls `chatgpt.com/backend-api/codex` with `chatgpt-account-id`, `originator: goat` and `session-id` headers. It is undocumented; keep its differences inside `provider/openai.rs` and `account/codex.rs`.
- Models come from the built-in catalog `crates/goat-core/src/catalog.toml`, not from provider model lists. Add a model there when a provider releases one; each entry has an id, a provider, and optionally a name, effort levels and a default effort. A test checks that each provider's default model is in it.
- Tool schemas are strict JSON Schema for OpenAI: every property is listed in `required`, and optional ones are nullable.
- The SSE fixtures in `crates/goat-core/tests/fixtures` follow the documented event shapes. They are not recordings of real responses.
- The model has one tool, `run`: JavaScript in an embedded QuickJS sandbox (`tool/script.rs`). The sandbox reaches the system only through `tool/host.rs`, so every shell command passes `danger.rs`. Do not give it other ways out.
- `Script` declares `finish` before `context` and `runtime`. Fields drop in order, and QuickJS aborts the process when a runtime is freed while a saved value is still alive.
- Before `edit` or `write` changes a file, `host.rs` records its old content through `tool::Journal` into the `changes` table, synchronously, so an interrupted turn cannot lose the record. `changes` is append-only and keyed by time, not by message: rewinding the code to a prompt takes, for each path, the earliest row after that prompt, and restoring is itself recorded. Rewinding the conversation deletes the messages from that prompt on; keep the cut at a user prompt so both providers accept what remains.
- `~/.goat/goat.db` stores credentials with mode 0600. Tests use `Store::open_in_memory` or a temporary directory, never the real home.

## Boundaries

- Always: add or update a test with every behavior change.
- Ask first: new dependencies, schema migrations, and changes to the blocked commands in `tool/danger.rs`.
- Never: put real keys or tokens in code, tests or fixtures, or call real model APIs from tests.

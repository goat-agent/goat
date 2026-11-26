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

- `crates/goat-core` is the library: agent loop (`session`), providers (`provider/openai`, `provider/anthropic`), the `run` tool and its sandbox (`tool/`), language servers (`tool/lsp`), SQLite store (`store`), accounts with ChatGPT sign-in (`account`), MCP servers (`mcp`), skills (`skill`), agent definitions (`agent`, with the built-in ones in `crates/goat-core/agents/`) and built-in plugins (`plugin`, with their files in `crates/goat-core/plugins/`). It never prints, reads environment variables or touches the terminal.
- `crates/goat-cli` builds the `goat` binary: the inline input box (`input`), output (`render`), and the commands (`chat`, `login`).
- `docs/style/rust.md` is the binding code style. Read it before writing code.

## Rules

- No comments in code. The only exceptions are listed in `docs/style/rust.md`.
- Keep memory low. Stream instead of buffering, and keep large data on disk. `crates/goat-core/tests/memory.rs` fails on Linux when 50 turns peak above 40 MiB, not counting the pages of the program's own code.
- A failed tool call is an `Outcome` with `is_error`, not an `Err`.
- The README is a short overview for newcomers. Do not describe features, commands or behavior there; the code and its tests are the reference.
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
- Before each prompt, `session/snapshot.rs` snapshots the working directory with git into `~/.goat/snapshots/<key>/repo`, with its own index per session, the user's and the system's git config shut out, and never the project's `.git`. The tree id is kept in the `snapshots` table under the prompt's message id, so rewinding the conversation, which deletes the messages from that prompt on, drops their snapshots too. Restoring the files makes the working directory match the prompt's tree; ignored files and files over 10 MB are never in a tree, so they are never touched. Keep the conversation cut at a user prompt so both providers accept what remains.
- MCP is configured only by the user: `mcp.toml` files plus `goat mcp`. A project's `.goat/mcp.toml` loads only after the user allows its exact content (`mcp_reviews`, path and sha256); `goat mcp add`/`remove` keep a file allowed only if it was allowed before the edit. Vendor-specific integrations belong in plugins, not in `mcp`.
- MCP OAuth is rmcp's `auth` feature. Sign-ins live in `logins` under `mcp:<url>`, as rmcp's `StoredCredentials` JSON, read on every request. rmcp needs a `Send + Sync` store, so it goes through `Store::logins`, a second connection to the same database; plugins share the table under `plugin:<name>`.
- Plugins are built in: each folder in `crates/goat-core/plugins/` is added to `BUILTIN` in `plugin.rs` with `include_str!`, and a test loads every one. A plugin's servers sign in under the same `mcp:<url>` keys as the user's own, so a plugin needs no storage of its own. Names of servers, plugins and accounts use letters, digits, `-` and `_`; `.` separates a server from its tool and `#` a server from its account. A service with its own remote MCP server that lets clients register is a plugin of only `plugin.toml` and an `mcp.toml` with its `url`, without a skill: `mcp.tools()` puts the server's `instructions` first, so the service's own guide reaches the model.
- Tools come only from MCP servers. When a service has no usable MCP server, goat ships one: `mcp/builtin/` runs it in the same process over an in-memory pipe, and an `mcp.toml` entry names it with `builtin = "gmail"`. The pipe uses the `initialize` handshake, because rmcp's in-process server waits for it. `mcp/builtin/gmail.json` is the tool contract of Google's own Gmail MCP server (names, input schemas, annotations), so a plugin can switch to `url = "https://gmailmcp.googleapis.com/mcp/v1"` once that server is generally available without the model noticing. The other Google plugins (`google_calendar`, `google_drive`, `google_docs`, `google_sheets`, `google_slides`, `google_contacts`) work the same way: each `mcp/builtin/<product>.json` is the `tools/list` of Google's own server for that product without `outputSchema`, each server asks for the scopes that server asks for, and `mcp/builtin/google.rs` holds what they share (sign-in, REST calls, one rmcp server for all of them). Keep their answers in the shape of Google's `outputSchema`.
- The computer plugin's server is `mcp/builtin/computer.rs`. It drives the desktop only through the `Platform` trait in `computer/desktop.rs`, on one `goat-desktop` thread; tests use `computer/fake.rs`. `computer/macos.rs` is the macOS adapter; it and `apple/macos.rs` are the only modules that may use `unsafe`. Its live checks are `#[ignore]` tests that open and quit Calculator: `cargo test -p goat-core --lib the_real_ -- --ignored --test-threads 1`. The user approves each app once per session or always (`grant:computer:<bundle id>` in `logins`), the terminal goat runs in is never acted on, and `~/.goat/computer.lock` keeps a second session off the desktop. `computer::Escape` is a listen-only event tap that ignores goat's own keys; the CLI stops a turn on Esc only after the turn has called `computer.*`.
- The apple plugin's server is `mcp/builtin/apple.rs`. It reaches Calendar, Reminders and Contacts only through the `Library` trait in `apple/library.rs`, on one `goat-apple` thread from `mcp/builtin/worker.rs`, which the computer's desktop thread uses too; tests use `apple/fake.rs`. `apple/macos.rs` is the EventKit and Contacts adapter. Its live checks are `#[ignore]` tests that create and delete a calendar, a reminder and a contact in the user's real accounts: `cargo test -p goat-core --lib live_library -- --ignored --test-threads 1`. The event and contact tools in `apple.json` use the names and shapes of the Google calendar and contacts contracts, and a test checks that shared parameters keep the same types; both servers read times through `mcp/builtin/zone.rs` and arguments through `mcp/builtin/args.rs`. The default time zone is macOS's own (`NSTimeZone`), the one EventKit uses, because jiff cannot always find it. The tools of an app whose macOS permission is missing are left out of the list. An occurrence of a repeating event has the id `<event id>_<start as yyyymmddThhmmssZ>`, like Google's.
- An agent is a `Session` with `Depth::Agent`, started by `agent()` in a run through `session/agent.rs`. Its session row points at the assistant message that called it (`spawned_by`), so rewinding the parent deletes it and `/resume` never lists it. Agents share the parent's `Shared` context (MCP clients, language servers, skills); an agent's run cannot start agents. Its instructions are the parent's, then `prompt::AGENT`, then the definition's body; an agent's `model` is a catalog id and resolves to an account like `/model`, and a model no account offers is an error, never a silent fallback.
- Language servers are external programs from `tool/lsp/servers.toml` and `~/.goat/lsp.toml`, found in the project's `node_modules/.bin` or the `PATH`, never downloaded. Each entry says whether the server pushes its diagnostics or is pulled; advertising pull makes pyright and sourcekit-lsp stop pushing, so only pull entries advertise it. After a run that changed files, `Servers::report` syncs them, waits by the rules in `tool/lsp/client.rs` (a fresh publish, no progress opened since the change, 300 ms of quiet, 1.5 s without any message means no change, 10 s at most; a young pull server is pulled again after its first `workspace/diagnostic/refresh`) and appends the new errors to the run's result. Their live checks are `#[ignore]` tests: `cargo test -p goat-core --lib tool::lsp::tests::the_real_ -- --ignored --test-threads 1`.
- A tool annotated `destructiveHint: true` runs only after the user allows it: the host sends `Step::Confirm` and waits for the answer. A run asks one question at a time, and `Decision::Run` allows that tool for the rest of the run. Questions from agents and MCP elicitations can be open at once, so a sink queues them and never drops one; an answer that is dropped reaches the model as unanswered, not as refused. Commands and file operations may not touch `~/.goat`.
- `~/.goat/goat.db` stores credentials with mode 0600. Tests use `Store::open_in_memory` or a temporary directory, never the real home.

## Boundaries

- Always: add or update a test with every behavior change.
- Ask first: new dependencies, schema migrations, and changes to the blocked commands in `tool/danger.rs`.
- Never: put real keys or tokens in code, tests or fixtures, or call real model APIs from tests. The one exception is goat's desktop OAuth client in `mcp/builtin/google.rs`: Google treats an installed app's client secret as public, as gcloud does.

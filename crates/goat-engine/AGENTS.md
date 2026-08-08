# AGENTS.md — goat-engine

`CodingEngine` is the production `Engine`. The crate is split by responsibility; `lib.rs` is the
shared-types hub (`CodingEngine`, `run()` op loop, `SessionServices`/`SessionContext`, `SessionState`,
`Run`/`Report`/`TurnIds`, `LoopEnv`, `Flow`) and every module imports from it.

`SessionContext` is a newtype over `Arc<SessionServices>` that derefs to it, and `LoopEnv` owns its provider, target,
tool defs and cwd rather than borrowing them. Both are owned on purpose: a detached run outlives
the turn that started it, so anything it reads has to be `'static` and `Send`. The one interior
mutability is `SessionServices::registry` (`Mutex<Arc<Registry>>`, swapped wholesale by login and account
removal — never mutated in place). `SessionState` bundles the four mutable per-session fields
(`target`, `conversation`, `tracker`, `thread_id`) threaded through the turn lifecycle; it stays
outside `SessionContext` because a background run must not touch it.

## Modules

| Module | Owns |
|---|---|
| `prompt` | `SYSTEM_PROMPT`, system-prompt assembly, skill listing |
| `accounts` | login/account lifecycle, model discovery, per-account registries, `provider_for` |
| `threads` | thread listing/rename/resume, stored-message parsing |
| `persist` | every goat-store write: threads, turns, messages, tool calls, `now_ms` |
| `turn` | `handle_turn` (top-level turn lifecycle, mid-turn op select loop), `handle_idle_op`, `handle_shell`, `handle_compact`, `SessionState`, `TurnEnd` |
| `rounds` | `core_loop`, `run_round` (provider stream consumption), `process_round_output` |
| `tools_exec` | tool defs, parallel tool batches, `execute_tool` routing, display helpers |
| `delegate` | the `Subagent`/`SubagentKill` tools: spec resolution, child runs, detaching, concurrency cap |
| `ask` | the `Ask` tool: question schema, blocking answer channel |
| `subagent` | `SubagentSpec`/`SubagentRegistry` (built-ins + `~/.goat/subagents/*.md` + `.goat/subagents/`) |
| `background` | `background::Runs`: one registry of detached runs over `Kind::{Bash, Subagent}`, and the wake it raises |
| `bash_tools` | `BashOutput`/`BashInput`/`BashKill`, the `Bash` schema augmentation, the running-run roster |
| `instructions` | AGENTS.md discovery and injection |
| `rate_limit_cache` | rate-limit snapshot persistence |
| `shell` | `<shell-input>`/`<shell-output>` history encode/decode for `SubmitShell` |
| `websearch` | the engine-level `WebSearch` tool (provider `web_search`) |
| `conversation` | the `Conversation` history (messages + db row ids) |
| `retry` | exponential-backoff retry over classified provider errors |
| `compaction` | `ContextTracker` budget and LLM-summarization auto-compaction |

## Dependency direction

`turn → rounds`; `rounds → tools_exec → {delegate, ask, bash_tools}`; `delegate → rounds::core_loop`
is the one intentional back-edge (the delegation recursion itself, boxed). That back-edge is why
`delegate::run_child` returns an explicitly boxed `Send` future instead of an `async fn`: the cycle
`run_delegation → detach → run_child → core_loop → run_delegation` gives `Send` inference nothing to
anchor on, and `tokio::spawn` needs the bound. `turn`/`threads`/`accounts` lean on `persist`;
`accounts` and `threads` are otherwise leaves. Engine integration tests live in `lib.rs`; unit tests
sit next to what they exercise.

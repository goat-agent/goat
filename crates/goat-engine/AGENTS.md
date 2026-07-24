# AGENTS.md — goat-engine

`GoatAgent` is the production `Engine`. The crate is split by responsibility; `lib.rs` is the
shared-types hub (`GoatAgent`, `run()` op loop, `Ctx`, `SessionState`, `Run`/`Report`/`TurnIds`,
`LoopEnv`, `Flow`) and every module imports from it. `Ctx` bundles the immutable shared services
for a turn; `SessionState` bundles the four mutable per-session fields (`target`, `conversation`,
`tracker`, `thread_id`) threaded through the turn lifecycle.

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
| `delegate` | the `Agent` tool: spec resolution, child runs, concurrency cap |
| `ask` | the `Ask` tool: question schema, blocking answer channel |
| `agent` | `AgentSpec`/`AgentRegistry` (built-ins + `.goat/agents/*.md`) |
| `instructions` | AGENTS.md discovery and injection |
| `rate_limit_cache` | rate-limit snapshot persistence |
| `shell` | `<shell-input>`/`<shell-output>` history encode/decode for `SubmitShell` |
| `websearch` | the engine-level `WebSearch` tool (provider `web_search`) |
| `conversation` | the `Conversation` history (messages + db row ids) |
| `retry` | exponential-backoff retry over classified provider errors |
| `compaction` | `ContextTracker` budget and LLM-summarization auto-compaction |

## Dependency direction

`turn → rounds`; `rounds → tools_exec → {delegate, ask}`; `delegate → rounds::core_loop` is the
one intentional back-edge (the delegation recursion itself, `Box::pin`ned). `turn`/`threads`/
`accounts` lean on `persist`; `accounts` and `threads` are otherwise leaves. Engine integration
tests live in `lib.rs`; unit tests sit next to what they exercise.

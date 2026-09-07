# AGENTS.md — goat-engine

`CodingEngine` is the production `Engine`. The crate is split by responsibility. `lib.rs` is the
shared-types hub — `CodingEngine`, the `run()` op loop, `SessionServices`/`SessionContext`,
`SessionState`, `Run`/`Report`/`TurnIds`, `LoopEnv`, `Flow` — and every module imports from it.

## Ownership

`SessionContext` is a newtype over `Arc<SessionServices>` that derefs to it. `LoopEnv` owns its
provider, target, tool defs and cwd rather than borrowing them.

Both are owned because a detached run outlives the turn that started it, so anything it reads must be
`'static` and `Send`.

The one interior mutability is `SessionServices::registry`, a `Mutex<Arc<Registry>>` swapped wholesale
by login and account removal. Never mutate it in place.

`SessionState` bundles the four mutable per-session fields (`target`, `conversation`, `tracker`,
`conversation_id`) threaded through the turn lifecycle. Keep it outside `SessionContext`: a background
run must not touch it.

## Modules

| Module | Owns |
|---|---|
| `prompt` | `SYSTEM_PROMPT`, system-prompt assembly, skill listing |
| `accounts` | login/account lifecycle, model discovery, per-account registries, `provider_for` |
| `conversations` | conversation listing/rename/resume, stored-message parsing |
| `persist` | every goat-store write: conversations, turns, messages, tool calls, `now_ms` |
| `turn` | `handle_turn`, `handle_idle_op`, `handle_shell`, `handle_compact`, `SessionState`, `TurnEnd` |
| `rounds` | `core_loop`, `run_round` (provider stream consumption), `process_round_output` |
| `tools_exec` | tool defs, parallel tool batches, `execute_tool` routing, display helpers |
| `delegate` | the `Subagent`/`SubagentKill` tools: spec resolution, child runs, detaching, concurrency cap |
| `ask` | the `Ask` tool: question schema, blocking answer channel |
| `subagent` | `SubagentSpec`/`SubagentRegistry` (built-ins + `~/.goat/subagents/*.md` + `.goat/subagents/`) |
| `background` | `background::Runs`, one registry of detached runs over `Kind::{Bash, Subagent}`, and the wake it raises |
| `bash_tools` | `BashOutput`/`BashInput`/`BashKill`, the `Bash` schema augmentation, the running-run roster |
| `instructions` | AGENTS.md discovery and injection |
| `rate_limit_cache` | rate-limit snapshot persistence |
| `shell` | `<shell-input>`/`<shell-output>` history encode/decode for `SubmitShell` |
| `websearch` | the engine-level `WebSearch` tool (provider `web_search`) |
| `conversation` | the `Conversation` history: messages plus db row ids |
| `retry` | exponential-backoff retry over classified provider errors |
| `compaction` | `ContextTracker` budget and LLM-summarization auto-compaction |

## Dependency direction

`turn → rounds`, then `rounds → tools_exec → {delegate, ask, bash_tools}`.

`delegate → rounds::core_loop` is the one intentional back-edge, the delegation recursion itself,
boxed. That is why `delegate::run_child` returns an explicitly boxed `Send` future instead of an
`async fn`: the cycle `run_delegation → detach → run_child → core_loop → run_delegation` gives `Send`
inference nothing to anchor on, and `tokio::spawn` needs the bound.

`turn`, `conversations` and `accounts` lean on `persist`. `accounts` and `conversations` are otherwise
leaves.

Engine integration tests live in `lib.rs`; unit tests sit next to what they exercise.

## The `Engine` trait

Its only method takes `self` by value, so it is not callable through a trait object. Nothing uses
`dyn Engine`.

Decoupling comes from generics plus bounded `tokio::mpsc` channels (32 ops, 512 events) carrying
`goat-protocol`. The trait avoids `async_trait` and `Stream`; both are used freely elsewhere.

## Backgrounding is a flag, not a tool family

`Bash(background=true)` and `Subagent(background=true)` return a run id instead of a result. There is
no `ProcessStart`.

`tools_exec` intercepts the backgrounded `Bash` call, so `goat-tool-shell` stays a plain synchronous
leaf that knows nothing about the registry. `build_tool_defs` adds the `background`/`watch` switches
to the `Bash` schema only when `allow_delegate`, so a subagent is never offered them.

The remaining verbs are `BashOutput`, `BashInput`, `BashKill` and `SubagentKill`. **Do not add a list
tool**: `roster_message` injects the running set every top-level round, so a list would turn
something the agent is already told into something it must remember to ask for.

### `background::Runs` is one registry over two kinds

`Kind::{Bash, Subagent}` share one id space, so `#3` is unambiguous.

| Shared | Per kind |
|---|---|
| ids, state, the wake trigger, already-seen bookkeeping, `roster`, `kill`, `shutdown_all` | `Detail::Bash` holds the ring buffer, stdin and process group; `Detail::Subagent` holds the report and its `CancellationToken` |

Keep `Event::ProcessListChanged` bash-only. A background subagent already reaches the TUI as
`SubagentStarted`/`SubagentDone`, so listing it would double-count it. The roster and the wake read
`roster()` / `take_pending_observations()`, which cover both kinds.

The `Event::Process*` family keeps its name because those events really do carry a pgid, an exit code
and stdout/stderr. The id they share with subagent runs is `RunId`, not `ProcessId`.

### Waking, and why nothing polls

Every background run wakes the agent when it finishes. `watch` only adds wakes for output while a
bash run is still going, so waiting is never a reason to set it. The tool descriptions send a waiting
agent to end its turn rather than re-read `BashOutput`.

**Never reintroduce a blocking read or a wait timeout.** That is the anti-polling design.

A wake is suppressed exactly when the agent already knows: it read the exit through `BashOutput`, or
it stopped the run itself with `BashKill` / `SubagentKill`.

Keep `BashOutput` even though the wake exists. A run that never exits (`pnpm dev`) fires no wake, and
a `watch` flood auto-clears `watched` (`WATCH_FLOOD_LINES`), which would otherwise leave its output
unreachable.

### A detached subagent outlives its turn

`delegate::detach` takes no `CancellationToken` parameter; it mints a fresh one owned by the registry
entry. An interrupt on the parent turn therefore cannot reach it, and only `SubagentKill` and
`shutdown_all` can.

The `MAX_CONCURRENT_SUBAGENTS` permit is acquired *inside* the spawned task, not before detaching, so
a full pool delays a background run instead of blocking the turn that started it.

## History and compaction

`code_messages` is insert-only; compactions live in `code_compactions`. `/resume` rebuilds engine
history from the **latest compaction alone**, while the transcript replays full scrollback with a
marker per compaction.

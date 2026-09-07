# AGENTS.md — goat-store

The agent's tables. One database file, `~/.goat/goat.db`, but **four crates own migrations against
it**:

| Crate | Tables |
|---|---|
| `goat-store` | `personas`, `conversations`, `messages`, `core_memory`, `episodic_memory`, `semantic_memory`, `signal_log`, `tool_invocations`, `model_scores`, `scheduled_tasks`, `task_runs`, `conversation_summary`, `runtime_flags`, `goals`, `integration_state`, `integration_observations`, `agent_activity` |
| `goat-memory` | `facts`, `mem_index`, `recall_stats`, `mem_index_meta` |
| `goat-code-store` | everything `code_`-prefixed |
| `goat-proxy-store` | `proxy_requests`, `proxy_rate_limits` |

`code_` and `proxy_` are namespaces. The agent and memory tables are unprefixed, so check a new agent
table against the memory list before adding it.

## Migrations are append-only

Never edit or delete an applied migration; `sqlx::migrate!` checksums them. Express a removal as a new
migration. This holds in all four crates.

## Insert-only tables

`code_messages` is insert-only, and compactions live in `code_compactions`; see
`crates/goat-engine/AGENTS.md` for how `/resume` reads them back.

`integration_observations` is likewise lossless. The `observation` agent tool resolves
`observation:<id>` against it, so rewriting a row breaks a citation that already shipped.

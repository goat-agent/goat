# AGENTS.md — goat-memory

**Memory is not per-agent.** It is one global tree at `~/.goat/memory/<scope>/`, keyed by `Scope`
(`owner`, `self`, `domain/<name>`). `agents/<slug>/` holds only `agent.md`, `config.json` and that
agent's `skills/`.

The agent consolidates memory nightly at 04:00.

## Dependency direction

This crate defines its own `Embedder` trait and does not depend on `goat-embedding`, which is
agent-side and reaches memory through `goat-runtime`'s adapter. Keep the arrow pointing that way.

Embedding settings are per-agent declarations selecting one global index configuration, enforced at
boot and reload; see `crates/goat-runtime/AGENTS.md`.

## Tables

This crate owns `facts`, `mem_index`, `recall_stats` and `mem_index_meta` in the unprefixed namespace
it shares with `goat-store`; see `crates/goat-store/AGENTS.md`.

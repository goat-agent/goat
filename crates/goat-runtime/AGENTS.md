# AGENTS.md — goat-runtime

The agent runtime: boot, the `Supervisor`, explicit tool registration, and the config-apply path.

## `goat reload` applies config; writing the file does not

The `Supervisor` holds one `CancellationToken` child per agent plus that agent's `config.json`
fingerprint. A reload re-reads config, validates it, and respawns only the agents whose own config
changed.

A change to the top-level `config.json`'s `integrations` or `providers` rebuilds the shared world —
provider registry, connections, integration tools — and respawns every agent, because the
`ToolRegistry` handed to each `Brain` is immutable once built.

**Validation failure replaces nothing.** Running agents keep the settings they already had.

Ask the filesystem which agents exist rather than trusting `scan_agents`, which silently drops an
agent whose `config.json` stopped parsing. A directory still holding an `agent.md` is a load failure
to report, not a removal to act on.

The trigger is `admin.agent_reload`. Every CLI that writes config calls it afterwards, so nothing
tells the user to restart the daemon. Only a new binary or a changed global embedding configuration
still needs one.

## What a respawn interrupts

| | |
|---|---|
| Survives | the turn in flight. `Brain::run` awaits `handle_turn` inside a `tokio::select!` arm body, and a chosen arm runs to completion, so cancelling the token is only observed on the next loop. |
| Is interrupted | the channel pump. Inbound messages during the swap can be lost. |

`agent.md` and skills are re-read every turn (`Brain::agent_definition`, `SkillSet::load`), so neither
takes part in a reload. The `AgentCard` loaded at boot is only the fallback for a failed read.

## Embedding is declared per agent, but selects one global index

Every memory-enabled agent that declares embedding must name the same provider and model. Boot rejects
conflicts deterministically.

Reload rejects a conflict, or a change from the running configuration, **before replacing any live
settings**, because the global `MemoryEngine` is built at boot.

Only `openai` is implemented. An unsupported provider or a failed probe leaves vector recall disabled
while core and full-text memory keep working. The stored embedding identity includes provider and
model, so changing either rebuilds the derived vector index even when dimensions match.

## Explicit registration lives here

Agent tools needing injected runtime deps are wired by explicit `register()` calls in this crate:
`goal`, `memory`, `pty`, `code`, `schedule`, and `goat_agent_tool_browser::register` (which needs the
`CodeSessionHub`). Only `fs`/`shell`/`skill` use `inventory` + `pub const NAME: ToolName`.

## Also here

| Module | Does |
|---|---|
| `validate_watch` | resolves a watch query against a leaf's `WatchVocabulary` alone, needing no store, bus or network. See `crates/goat-integration/AGENTS.md`. |
| `channel_secrets` | moves a declared slot found in `config.json` into the credential store and rewrites the file. See `crates/goat-channel/AGENTS.md`. |
| `layout` | boot migration of the old filesystem layout (loose `agents/*.md`, `profiles/<slug>/skills/`). |

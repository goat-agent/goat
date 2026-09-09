# AGENTS.md — goat-config

`GoatPaths` in `paths.rs` lays out `~/.goat/`; `HOME` is the only thing that moves it. Read
`paths.rs` rather than duplicating the list.

## Config files, split by who reads them

| File | Owner | Keys |
|---|---|---|
| `~/.goat/config.json` | the daemon | `search`, `web_fetch`, `proxy`, `integrations`, `providers`, `devices` |
| `~/.goat/client.json` | the client | `theme`, `mouse_capture_enabled`, `remotes`, `default_remote` |
| `~/.goat/desktop.json` | the desktop app | recent `projects` with paths and added timestamps |

One file with two owners is why "who writes this" had no principled answer.

A first load with no `client.json` adopts the four client keys out of `config.json` **and removes
them from it**, so a key never lives in two places. Adoption takes only the keys the client owns; the
daemon's stay behind.

## The daemon's config has one writer: `admin.config_edit`

It takes a list of `ConfigEdit`, a closed set of intents (`provider_set`, `search_default_set`,
`integration_remove`, …). The daemon is the only place that opens the file.

One method rather than nine keeps the method table small, and the enum is the vocabulary. Pass
provider-shaped payloads (a search account, an integration entry) as **opaque JSON**, so `goat-api`
never learns a concrete provider name.

`goat provider`, `goat code search` and `goat agent integration` call it. A local target autostarts
the daemon, so a write works from a cold machine.

Two writers bypass it, both deliberately:

| Writer | Why |
|---|---|
| `goat mcp` | its secrets and `mcp.json` move as a pair with rollback. See `crates/goat-mcp/AGENTS.md`. |
| `goat remote` | device key material is read by the client, so the client owns it. See `crates/goat-remote/AGENTS.md`. |

## Applying config

Writing the file changes nothing. `goat reload` applies it; see `crates/goat-runtime/AGENTS.md`.

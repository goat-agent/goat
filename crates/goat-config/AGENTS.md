# AGENTS.md — goat-config

`GoatPaths` in `paths.rs` lays out `~/.goat/`; `HOME` is the only thing that moves it. Read
`paths.rs` rather than duplicating the list.

## Config files, split by who reads them

| File | Owner | Keys |
|---|---|---|
| `~/.goat/config.toml` | the daemon | `search`, `web_fetch`, `proxy`, `integrations`, `providers`, `devices` |
| `~/.goat/client.json` | the client | `theme`, `mouse_capture_enabled`, `remotes`, `default_remote` |
| `~/.goat/desktop.json` | the desktop app | recent `projects` with paths and added timestamps |

`config.toml` is TOML because it is the one file users hand-edit: provider specs live in
`[providers.<id>]` tables and comments must survive daemon writes. Everything else under `~/.goat/`
stays JSON.

A first load with no `client.json` adopts the four client keys out of `config.toml` **and removes
them from it**, so a key never lives in two places. Adoption takes only the keys the client owns; the
daemon's stay behind.

## The daemon's config has one writer: `admin.config_edit`

It takes a list of `ConfigEdit`, a closed set of intents (`provider_set`, `search_default_set`,
`integration_remove`, …). The daemon is the only place that opens the file.

One method rather than nine keeps the method table small, and the enum is the vocabulary. Pass
provider-shaped payloads (a search account, an integration entry, a provider spec) as **opaque
JSON**, so `goat-api` never learns a concrete provider name.

`goat provider`, `goat code search` and `goat agent integration` call it. A local target autostarts
the daemon, so a write works from a cold machine.

Two writers bypass it, both deliberately:

| Writer | Why |
|---|---|
| `goat mcp` | its secrets and `mcp.json` move as a pair with rollback. See `crates/goat-mcp/AGENTS.md`. |
| `goat remote` | device key material is read by the client, so the client owns it. See `crates/goat-remote/AGENTS.md`. |

## Reads vs writes

`Config` is the typed read model: `Config::load_at` parses the whole file and ignores unknown keys.
`ProviderSpecs` is the thin `[providers]` reader the registry re-reads on every build.

`ConfigDocument` is the write path: a `toml_edit::DocumentMut` with surgical set/remove methods
(`set_provider`, `remove_provider`, `set_integration`, `set_search`, …) so daemon edits touch only
the section they own and user comments elsewhere survive. `json_to_toml_item` converts opaque JSON
payloads into TOML items; JSON `null` drops the key because TOML has no null.

## Applying config

Writing the file changes nothing. `goat reload` applies it; see `crates/goat-runtime/AGENTS.md`.

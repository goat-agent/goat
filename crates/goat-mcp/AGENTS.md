# AGENTS.md — goat-mcp

The MCP **protocol** crate: transports (stdio and streamable HTTP), session lifecycle, result
extraction, error classification, OAuth.

It knows neither tool system. Put tool adapters in `goat-mcp-tools`.

## Handshake

`handshake` is the only place that names a protocol revision. No other crate mentions an MCP version,
and a server's era is never configured — that would be a per-server quirk table.

It tries one era and retries in the other only when the failure could be the era itself. `PREFERRED`
picks which goes first. Today that is legacy (`2025-11-25`, the `initialize` handshake), because
every reachable server still speaks it, so the retry never fires and connecting costs one round trip.

Classify new failures in `handshake::sort`, the single place that reads meaning into an rmcp error:

| Failure | Retry the other era? |
|---|---|
| `-32022`, `NoCompatibleProtocolVersion` — proves a modern peer | no |
| transport, auth — era-agnostic | no |
| anything else | yes |

## OAuth: two rungs of rmcp's registration ladder

`run_login` takes a `ClientIdentity`.

| Identity | Behavior |
|---|---|
| `preregistered` set | wins outright |
| none | rmcp falls back to Dynamic Client Registration, which writes `integrations.<kind>.client_id` back into `config.json` |

Every leaf but the Google trio uses DCR. Declaring `.preregistered()` asserts that the authorization
server has no `registration_endpoint`, so `goat integration add` prompts for a client id and secret
first. That pair lives in `credentials.json` under the `client_id` / `client_secret` slots of the
same integration key, and disconnect removes both.

### The third rung, CIMD, is deferred

DCR is deprecated in the 2026-07-28 MCP spec and Client ID Metadata Documents replace it.
`mcp.linear.app`, `mcp.sentry.dev` and `mcp.notion.com` already advertise
`client_id_metadata_document_supported`; `mcp.atlassian.com` does not. All three still expose a
`registration_endpoint`, so nothing is forced yet.

Do these three in order before turning it on. rmcp picks the CIMD rung whenever a server advertises
it and does **not** fall back to DCR on failure, so a document that 404s breaks those three servers'
next login.

1. Add a callback port pool for MCP logins only, as a new `bind_loopback_in(ports)` beside
   `goat_auth::bind_loopback`. Do not change `bind_loopback`: it binds port 0 and cannot fail, and
   `goat-provider-gemini` and `goat-provider-anthropic` depend on that. Owning a fixed port is
   already the idiom for callers that need one (`goat-provider-openai-codex`, `goat-provider-xai`).
2. Add a test binding the document's `redirect_uris` to that port list and its `client_id` to the URL
   verbatim, so the pair cannot drift.
3. Serve the document at that HTTPS URL, before landing the constant that names it.

## `goat mcp` writes config directly

Its server secrets and `mcp.json` move as a pair with rollback, and `ConfigEdit` has no MCP
vocabulary. Splitting the pair across a process boundary would buy nothing and lose the rollback.
Everything else goes through `admin.config_edit`; see `crates/goat-config/AGENTS.md`.

Project-scope `goat mcp` servers stay code-only. The agent has no working directory, so
`load_user_manager` cannot see them.

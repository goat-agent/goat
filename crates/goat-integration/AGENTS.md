# AGENTS.md — goat-integration

The integration base: the watch query grammar (`query.rs`), the polling driver (`watch.rs`), the
shared parser skeleton (`shape.rs`) and the re-fire policies (`diff.rs`). Leaves live in
`goat-integration-<name>`; hosted-MCP leaves build on `goat-integration-mcp`.

## An integration owes neither tools nor a watch capability

Three shapes are all complete:

| Shape | Example |
|---|---|
| connection + tools + watch | most hosted-MCP leaves |
| connection + watch, no tools | `goat-integration-github`, reached through `shell` and `gh` |
| connection + tools, no watch | `goat-integration-posthog` |

Tools are normally discovered from a hosted MCP server's `list_tools`, not declared.

## Registration

Use `inventory` + `pub const ID` via `from_static(...)`. A leaf's `ctor` returns `service().build()`.
Every hosted-MCP integration is an `McpService` descriptor, and `McpIntegration` is the only
`impl Integration` among them. `goat-integration-github` implements the trait itself because it uses
no MCP.

## Auth

Connections are global. `IntegrationAuth` picks how one is established:

| Variant | Meaning |
|---|---|
| `Secret` | a pasted credential |
| `OAuth` | a round trip; mechanics live in `goat-mcp` |
| `External` | a host tool such as `gh` owns the credential, and the `config.json` entry is itself the connection marker |

## Per-agent binding

Put only connection-scoped keys in the agent's `integrations` map: `account`, `organization_slug`,
`user_id`, `host`. Watch policy keys belong in the `watch` section, and a stale one fails validation
with a pointer there.

Observations persist losslessly in `integration_observations`. The `observation` agent tool reads
them back, so a briefing citing `observation:<id>` resolves.

## Watch policy is a query DSL

Declare it in the **top-level `watch` section** of the agent's `config.json`: named workflows, each a
list of `{source, query, stream?}` entries. One driver task per workflow polls every source per tick
and publishes one merged `Event::WorkflowUpdate`, capped at 3 items with overflow counted.

| `watch` section | Result |
|---|---|
| absent | each bound integration's `default_watch` runs as its own single-source workflow |
| `"watch": {}` | everything disabled |
| present | **replaces** the defaults; it never merges |

Defaults are linear `assigned` → `assignee:@me is:open`, and github `review`/`assigned`.

Never change a default stream name: stream names key persisted `WatchState`. `limit:` is
resolver-reserved and `@me` is the only self-reference.

### Declaring a leaf's vocabulary

The grammar is closed. A leaf owns two things and must declare them together:

1. a static `WatchVocabulary` — which keys it understands;
2. a `compile_watch` hook — turning a resolved query into a `CompiledWatch`.

A leaf implementing `Integration` overrides `watch_vocabulary`. A hosted-MCP leaf passes the pair to
`McpService::watch(&VOCABULARY, compile)`. Neither path lets you set one without the other.

**Why:** `goat_runtime::validate_watch` resolves a query against the vocabulary alone, so it needs no
store, bus or network and runs in `goat doctor`, the config-writing CLIs and `goat reload`. A
vocabulary that lied about its compiler would make that validator wrong. `compile_watch` stays
authoritative and runs only when a plan is built.

### `Residue`: what a leaf does with tokens it does not know

| Policy | Leaves | Unknown key-value | Bare terms |
|---|---|---|---|
| `Residue::Keep` | github, slack, langfuse, atlassian | forwarded verbatim to the service's own search language | forwarded, so `TermPolicy` never fires |
| `Residue::KeepTerms` | sentry | rejected against its documented issue properties | forwarded |
| `Residue::Reject` | linear, notion, tiro, datadog, pagerduty, vercel | hard error | only here can `TermPolicy::Reject` refuse free text |

## Parsing: map through `shape`

| Helper | Does |
|---|---|
| `envelope` / `items` | unwrap whichever key a server wraps its list in, one level of nesting included |
| `more` | read whichever pagination flag it sends |
| `text` / `required` | pluck a field by candidate names, with dotted paths for nested objects |
| `squeeze` | clamp a summary |

Never re-derive an envelope key list in a leaf. A leaf needing no shaping beyond these needs no
`parse.rs` at all — atlassian, datadog, pagerduty and vercel map inline in `watch.rs`.

## The watch driver

`watch::run_workflow` is the only polling driver, and it knows nothing of rmcp. That is why
`goat-integration-github`, which shells out to `gh`, uses it too.

Diff state stays per source under the `(agent, integration, account, stream)` key, even inside a
multi-source workflow.

Pick one of `diff::{REBUILD, RETAIN, SETTLE}` rather than unifying them; they encode opposing intents.

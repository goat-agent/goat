# AGENTS.md — goat-integration-mcp

The shared base every hosted-MCP integration builds on. It registers nothing itself.

A leaf is an `McpService` descriptor plus its parser. `McpIntegration` is the only `impl Integration`
among them.

## Build a leaf with the const builder chain

Start at `McpService::new(...)`, chain `const fn` setters, and finish with `.build()` from the leaf's
`ctor`. Never hand-write the type.

| Setter | Declares |
|---|---|
| `.summary(text)` | the one line `goat integration list` shows |
| `.binding_keys(&[ConfigKey…])` | the usage keys the leaf reads; `host` is added for `ServiceUrl::FromHost` |
| `.secret(label, scheme)` / `.oauth(scopes)` | how a connection is established |
| `.preregistered()` | the auth server has no `registration_endpoint`, so the CLI prompts for a client id and secret |
| `.token_scheme` / `.env_var` / `.headers` | how the credential reaches the wire |
| `.call_timeout` / `.truncation_hint` | per-call limits |
| `.tools(policy)` | which tools the leaf exposes |
| `.identity(probe)` | how the connection names its account |
| `.watch(&VOCABULARY, compile)` | the watch pair, both halves at once |
| `.defaults(fn)` | `default_watch` streams |

`.watch` takes the vocabulary and the `compile_watch` hook together, and there is no way to set one
without the other. See `crates/goat-integration/AGENTS.md` for the grammar and why that pairing
matters.

## `ToolPolicy`

Use `ToolPolicy::all(prefix)` or `ToolPolicy::only(prefix, names)`, optionally `.deny(&[NameRule::…])`
with `Prefix`/`Suffix` rules.

The prefix namespaces a hosted server's tool names, so it is not cosmetic. Changing it renames every
tool the model has learned. It applies to the primary connection; any other connection's tools take
a prefix derived from its name (`goat_integration::tool_prefix`).

## Bindings

Read a leaf's slice of the agent's `integrations` map through `validate_binding::<T>` and
`read_binding::<T>`. Do not parse the binding `Value` by hand. The value is the connection's
config with the use layered on top, and `binding.account` is the connection name.

## Errors

`wire_error` and `classify` map an `McpError`, or a rendered string, into `IntegrationError`. Add a
case there rather than matching on error text in a leaf.

# AGENTS.md — goat-channel

A channel holds a resident connection under a bot identity and turns inbound traffic into
`IncomingMessage`. Leaves live in `goat-channel-<name>`.

## A channel owes no tools

It is a presence, not a reach. Workspace-wide search, and posting where the bot is not a member,
belong to the matching integration.

`slack` is both, and the two cannot be merged: `goat-channel-slack` is the bot people address
(`xoxb-` + `xapp-`, Socket Mode), while `goat-integration-slack` reaches in as the owner (`xoxp-`,
hosted MCP). Their token capabilities are disjoint, so neither is redundant.

## Registration

`inventory` + `pub const ID` via `from_static(...)`.

A `ChannelFactory` also carries `metadata: fn() -> ChannelMetadata`, declaring the display name, the
setup text, and one `SecretSpec` per secret the channel needs. The CLI drives its prompts off that
list, so **a channel that forgets its metadata gets asked for nothing.**

## Bindings and secrets

Bindings are per-agent, and **no secret ever lives in `config.json`.**

The `channels.<kind>` map records *that* an agent uses a channel. An empty object is a complete
binding, so never delete one for looking empty.

Every secret sits in `credentials.json` under
`{ service: channel, provider: <kind>, account: <agent slug>, slot: <secret name> }`. `slot` is the
axis letting one binding hold several secrets; `account` is the agent, not a workspace.

A boot that finds a declared slot in `config.json` moves it into the store and rewrites the file
(`goat-runtime::channel_secrets`). The stored value always wins over a stale config one.

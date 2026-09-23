# AGENTS.md — goat-mcp-tools

One neutral pair, `ResolvedTool` and `McpToolSource`, and one adapter over them:
`tools(resolved) -> Vec<Arc<dyn goat_tool::Tool>>`. The same `Arc<dyn Tool>` drops into the agent
registry (`goat-runtime`'s `register` block) and the code registry (`goat_tools::builtin_with` /
`CodingEngine::new`) — the unified `Tool` contract is what makes a single adapter possible.

**Add a source, not a pair of adapters.** Today the sources are `goat-integration-mcp`'s
`code_tools`/`register` for hosted integrations, and `from_manager` for `goat mcp` servers. A
source applies its own policy before the adapter sees a tool.

`McpToolSource::call` takes an optional `AgentId`, because a hosted integration resolves its
binding per calling agent. The adapter reads it from `ctx.agent` — `Some` when the agent's brain
invokes the tool, `None` in a code session. Sources with no such axis ignore it.

## Registration is global; selection is per-consumer

| Consumer | Gets |
|---|---|
| agent | the connections bound in its own `config.json`, plus every user-scope `goat mcp` server, filtered by its `tools` selectors |
| code session | every logged-in connection, or only those the project's `.goat/integrations.json` lists, and every user-scope server, unfiltered, because a person is driving it |

Project-scope `goat mcp` servers are code-only. The agent has no working directory.

A `ResolvedTool.enabled = false` lands in the registry but not in `default_specs()` (agent) or
`specs_for()` (code) — the code side surfaces those through the `ToolSearch` deferred catalog
instead.

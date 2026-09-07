# AGENTS.md — goat-mcp-tools

Both tool shells over one neutral pair, `ResolvedTool` and `McpToolSource`.

| Shell | Builds | For |
|---|---|---|
| `install` | `goat_agent_tool::ToolHandler` | the agent |
| `adapt` | `goat_tool::Tool` | code |

**Add a source, not a pair of adapters.** The shells stay at two however many sources exist. Today
they are `goat-integration-mcp`'s `code_tools`/`register` for hosted integrations, and
`from_manager` for `goat mcp` servers. A source applies its own policy before a shell sees a tool.

`McpToolSource::call` takes an optional `AgentId`, because a hosted integration resolves its binding
per calling agent. Sources with no such axis ignore it.

## Registration is global; selection is per-consumer

| Consumer | Gets |
|---|---|
| agent | the integrations bound in its own `config.json`, plus every user-scope `goat mcp` server, filtered by its `tools` selectors |
| code session | every connected integration and every user-scope server, unfiltered, because a person is driving it |

Project-scope `goat mcp` servers are code-only. The agent has no working directory.

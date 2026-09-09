# AGENTS.md — goat-agent-tool

The **agent-side** tool contract. The code engine has a separate one in `goat-tool`; the two are
parallel, not shared. A tool serving both goes through `goat-mcp-tools`.

`ToolHandler` is the trait, `ToolSpec` the declaration, `ToolCall`/`ToolOutput` the call shape.
`ToolName` is a `Cow<'static, str>` newtype, so an `inventory` registration can be `const`.

## Registration is split

`inventory` + `pub const NAME: ToolName` covers `fs`, `shell` and `skill` only. `goal`, `memory`,
`pty`, `code`, `schedule`, browser and computer tools need injected runtime deps, and are wired by explicit
`register()` calls in `goat-runtime`. Adding one of those means editing that crate, not just
declaring an inventory item.

## `ToolCaller`

Carries `agent: AgentId`, `agent_slug`, `conversation`, `goat_root`, `read_state`, and an optional
`audience`.

| Field | Rule |
|---|---|
| `audience` | `Principal(String)` or `Shared(String)`. `goat-runtime` maps it onto `goat_memory::Audience` — a principal when a specific user is speaking, global otherwise — so it scopes memory writes and recalls. |
| `goat_root` | Resolve paths only through `ToolCaller::resolve_path`, which calls `path::resolve_in_root`. The agent has no working directory, so a raw join has no root to be relative to. |
| `read_state` | A shared `HashMap<PathBuf, ToolReadSnapshot>` across tool calls, recording what a tool has already seen of a file. |

Pass `AgentId` through the constructor, never ambiently.

Tool results may contain text and base64 images. `text_for_model` is a text-only preview; the brain
must preserve the content blocks and error status when constructing provider tool results.

# AGENTS.md — goat-tool

The **tool contract** for both consumers: the code engine and the agent. One `Tool` trait, one
`ToolRegistry` type, one `ToolOutput` — each consumer fills the half of `ToolContext` it owns.

`Tool::call(&self, call: &ToolCall, ctx: ToolContext) -> ToolFuture` is the trait.
`ToolRegistry` holds `Arc<dyn Tool>` per name; the code session builds one through
`goat_tools::builtin_with`, the agent runtime builds another through explicit `register()` calls
in `goat-runtime`.

## `ToolContext` — who fills what

| Field | code session | agent turn |
|---|---|---|
| `sandbox` | the session `ToolSandbox` (cwd + policy) | a `ToolSandbox::rooted(goat_root)` |
| `cancellation` | the run's token | a fresh token (no turn cancel today) |
| `definition_context` | real `ToolDefinitionContext` | `Default` |
| `host`, `task`, `call` | `Some` | `None` |
| `agent` | `None` | `Some(&AgentContext)` — id, slug, conversation, audience, read_state |

`ToolContext` is `Copy` and `Deref`s to `ToolSandbox`, so `ctx.resolve(...)`, `ctx.cwd`,
`ctx.max_output_bytes`, `ctx.ensure_writable(...)` work on both sides. Agent tools read their
identity through `ctx.agent_context()?`; they are only ever invoked by the brain, which always
sets `agent`.

## Vary a spec through `ToolDefinitionContext`

That context is three flags: `interactive`, `top_level`, `planning`. A `Tool` may narrow both its
schema (`definition`) and its availability (`enabled`) from them.

`goat-tool-shell`'s background verbs gate on `top_level`, which is why a subagent is never offered
them.

`default_enabled()` is the agent-side axis: the agent registry's `default_specs()` filters on it
before the brain applies `tools` selectors (`selector_allows`, `validate_tool_selectors`).

## Never resolve a path by hand

Use `path::resolve_in_cwd`, `resolve_with_extra` or `resolve_with_policy` — or `ctx.resolve(...)`,
which is `resolve_with_policy` over the sandbox. `blocked_path` is the deny check;
`relative_display` is the display form.

A raw `cwd.join(raw)` escapes the sandbox on any `..`. That is the mistake this module exists to
prevent. The agent's sandbox root is `goat_root` instead of a session cwd — same resolver, same
guarantee.

## Output

`ToolOutput` is the MCP-shaped result: `content: Vec<ToolContent>` (text + base64 images),
`structured_content`, `is_error`, plus the code-side `summary`/`body`/`extensions` the TUI renders
via `ToolOutcome`. `text_for_model` is the text-only preview; `as_text` the first text part.
`truncate` caps a body by bytes, not chars. Return structure, not pre-formatted terminal text.

## One crate per domain, both shells inside

A `goat-tool-<domain>` crate holds the code impl at top level and the agent impl in `agent`
(`goat-tool-fs::agent`, `goat-tool-shell::agent`, `goat-tool-skill::agent`,
`goat-tool-browser::agent`, `goat-tool-computer::agent`). Agent-only domains are their own crate
(`goat-tool-goal`, `-memory`, `-observation`, `-schedule`, `-pty`, `-code`).

Agent shells that need daemon services take a small trait, not `CodeSessionHub`:
`goat-tool-browser::agent::AgentTransport`, `goat-tool-computer::agent::AgentTransport`,
`goat-tool-code::CodeDelegate` — all implemented by `CodeSessionHub` in `goat-daemon`, which keeps
the dependency arrow pointing down.

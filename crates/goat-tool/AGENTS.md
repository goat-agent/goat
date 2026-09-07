# AGENTS.md — goat-tool

The **code-side** tool contract. The agent has a separate one in `goat-agent-tool`; the two are
parallel, not shared.

`Tool::run(&self, input, ctx: &ToolSandbox)` is the trait. `ToolRegistry::builtin()` aggregates fs,
shell, search, skill and web. `goat-tool-browser` bypasses it; see that crate's `AGENTS.md`.

## Vary a spec through `ToolDefinitionContext`

That context is three flags: `interactive`, `top_level`, `planning`. A `Tool` may narrow both its
schema (`definition`) and its availability (`enabled`) from them.

`goat-tool-shell`'s background verbs gate on `top_level`, which is why a subagent is never offered
them.

## Never resolve a path by hand

Use `path::resolve_in_cwd`, `resolve_with_extra` or `resolve_with_policy` to turn a model-supplied
string into a `PathBuf`. `blocked_path` is the deny check; `relative_display` is the display form.

A raw `cwd.join(raw)` escapes the sandbox on any `..`. That is the mistake this module exists to
prevent.

`SandboxPolicy::is_read_only` decides whether a call may write. The shell sandbox itself is
`goat-sandbox`. The agent side resolves against `goat_root` instead of a cwd; see
`crates/goat-agent-tool/AGENTS.md`.

## Output

`truncate` caps a body by bytes, not chars. `display` and `gist` are the summary forms the TUI
renders. Return structure, not pre-formatted terminal text.

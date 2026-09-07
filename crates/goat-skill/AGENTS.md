# AGENTS.md — goat-skill

The only `SKILL.md` crate: one parser, one renderer, and every scope.

## Pick scopes, not a crate

| Constructor | Layers, lowest first |
|---|---|
| `Scopes::agent(root, slug)` | builtin < `~/.agents/skills` < `~/.goat/skills` < that agent's own |
| `Scopes::code(root, cwd)` | the same, with `<repo>/.goat/skills` as the last layer |
| `survey(root)` | every agent at once, for `goat doctor` |

`~/.agents/skills` is a skill scope outside the goat tree.

## Keep the dependency list at three

serde, thiserror and tracing. **Never depend on `goat-config` or `goat-protocol`.** A caller hands
this crate paths, so the agent tool does not inherit the engine vocabulary.

## `arguments` has two front ends

`goat-commands` turns it into a TUI slash command; `goat-agent-command-skill` turns it into a
Discord/Slack one. `value:` — `word`, `integer`, `choice`, `text_tail` — is mandatory, because both
front ends need to know what a value is.

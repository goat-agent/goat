# AGENTS.md — goat-command

The base for the **TUI** slash-command family, `goat-command-*`. Channel slash commands are
`goat-agent-command-*` and do not pass through here.

A command declares a `CommandSpec` and returns a `CommandEffect`. `goat-commands` aggregates the
built-ins in `CommandRegistry::builtin()` and layers skills on with `set_skills`, so a skill and a
built-in share one namespace and one parser.

## Declare the grammar; do not parse by hand

`CommandShape`, `BranchSpec`, `ParameterSpec` and `ParameterValue` describe the command. `parse_line`
turns raw input into a `CommandInvocation`.

Read values through the typed accessors — `text`, `integer`, `choice` — rather than re-splitting the
string. Render parse failures with `CommandParseError::message`, never with formatting inside a
command.

This is the grammar `goat-skill`'s `arguments` compiles into, which is why `value:` there is
mandatory: both front ends need to know what a value is.

## Reaching the daemon

Return `CommandEffect::Admin(Vec<AdminRequest>)` to change daemon-side state. It is a batch, so
several writes keep their order, and `goat-client`'s pump executes it.

Commands name credential keys but never construct a `CredentialStore`; see
`crates/goat-client/AGENTS.md`.

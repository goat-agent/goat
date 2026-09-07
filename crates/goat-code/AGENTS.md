# AGENTS.md — goat-code

The `goat code` binary: CLI entry, the headless bridge, and the `into_eyre` boundary bridging library
`thiserror` enums into this application's `color_eyre::Result`.

## Talking to the daemon

`goat code` always speaks to the resident daemon, which need not be on this machine. `goat remote`
names other hosts' daemons and `--remote <name>` attaches over mTLS.

It opens a new session by default. Only `-c` resolves cwd to the latest conversation. `-w` is refused
for remote targets, because worktrees are local git.

## Tests

Non-TUI binary paths (`--version`, `--help`, `update`, `--print-log-path`) are safe anywhere.

The headless bridge needs no tty; its codec round-trips and shutdown handshake are unit-tested in the
`headless` module.

`tests/browser_host` spawns the real `goat browser-host` process against a scripted Chrome on its
stdio, which is why it is pinned to `max-threads = 1` in `.config/nextest.toml`.

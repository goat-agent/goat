# AGENTS.md — goat-tui

The full-screen coding TUI. `goat-command-*` is the TUI slash-command family; channel slash commands
are `goat-agent-command-*`.

## Testing without a tty

The TUI needs a real tty, so do not drive it headlessly. Test the pure `App::update` reducer here, and
the engine's `Op → Event` behavior in `goat-engine`. Tests are inline `#[cfg(test)]` modules and are
concentrated in these two crates.

`App::new` takes an `Origin`, so a remote session is testable without a daemon. That is how the
header's local git/gh chrome is proven off for remote targets.

## No credential store here

`goat-tui` and `goat-command-*` name credential keys but never construct a `CredentialStore`, so a
client-side write is unrepresentable rather than merely absent. Reach the daemon through
`CommandEffect::Admin`; see `crates/goat-client/AGENTS.md`.

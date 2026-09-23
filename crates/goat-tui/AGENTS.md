# AGENTS.md — goat-tui

The full-screen coding TUI. `goat-command-*` is the TUI slash-command family; channel slash commands
are `goat-agent-command-*`.

## Testing without a tty

The TUI needs a real tty, so do not drive it headlessly. Test the pure `App::update` reducer here, and
the engine's `Op → Event` behavior in `goat-engine`. Tests are inline `#[cfg(test)]` modules and are
concentrated in these two crates.

`App::new` takes an `Origin`, so a remote session is testable without a daemon. That is how the
header's local git/gh chrome is proven off for remote targets.

## Input routing

A key flows in one order: the active `Screen`, then the composer menu, then `ctrl_key`, then
`on_normal_key`. The `/` and `@` menus are not screens — `App::composer_menu` is derived state
recomputed from the composer text at the end of every `App::update`/`App::on_key`, so it cannot
desync from the input. `Composer::revision` bumps on every content mutation; Esc latches the menu
closed at the current revision until the text changes again.

`InputOutcome::Ignored` falls through to the composer only from a
`Placement::Panel { composer_focused: true }` screen — and mouse events, which self-gate through
`wheel_scroll_allowed`/`selection_allowed`. Any other screen swallows what it does not handle, so a
modal never leaks keys into the composer and the menu never clobbers it. A `Panel` screen that
`fit_stack` clipped to zero rows (`App::panel_visible`, set by `view::render`) is bypassed entirely,
and a dormant menu under an active screen gets no input either.

## No credential store here

`goat-tui` and `goat-command-*` name credential keys but never construct a `CredentialStore`, so a
client-side write is unrepresentable rather than merely absent. Reach the daemon through
`CommandEffect::Admin`; see `crates/goat-client/AGENTS.md`.

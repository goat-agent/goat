# AGENTS.md — goat-desktop

The Tauri v2 desktop coding client, agent chat client, and local `host.computer` provider. It uses
only the local socket. The daemon executable is a sibling `goat` in development, otherwise
`~/.goat/bin/goat`, matching the CLI installer. It does not read remote-client preferences.

## Build and signing

macOS 14 or newer. Install Bun and `cargo install tauri-cli --locked`, then:

```
bun install --cwd crates/goat-desktop/ui
bun run --cwd crates/goat-desktop/ui build
cargo build -p goat-desktop
```

Run `cargo tauri dev` from this crate for webview development. Build an installed app with
`cargo tauri build` from this crate. The CLI must be installed separately for daemon autostart.

Local bundles use the stable code-signing identity `goat-desktop`. Create it once in Keychain
Access: Certificate Assistant → Create a Certificate → name `goat-desktop`, identity Self Signed
Root, certificate type Code Signing. Keep this certificate and private key across rebuilds.
Accessibility and Screen Recording permissions are tied to that identity. A development executable
launched from a terminal is not the installed signed app and can inherit its launcher's privacy
attribution or require different grants.

CI passes `--config tauri.ci.conf.json` to use ad-hoc signing. Do not name this file
`tauri.macos.conf.json`: Tauri automatically merges platform-named configuration, which would also
replace the local stable signing identity. CI bundles are not notarized.

`ui/dist/index.html` is a tracked minimal compile-time entry so workspace Rust builds need no Bun
step. All other dist contents are ignored. The app bundler always runs the real frontend build.
Restore the minimal entry after local builds rather than committing generated asset references.
`ui/src/lib/api.d.ts` is also generated and ignored; `bun run types` derives it from the frozen
`goat-api` schema. Never manually declare protocol shapes in the UI.

## Ownership

| Module | Owns |
|---|---|
| `sessions` | Attachments, task submission receipts, admin requests, slash commands, close/rebind |
| `state` | Rust event reducer and the `goat_command::Session` implementation |
| `adapter` | Serializable admin facade and fixed desktop settings/composer/viewport adapters |
| `backend` | Local API connection, atomic project list, workspace metadata and agent streams |
| `computer` | Native provider connection, permissions, global stop and activity overlay |
| `ui/src/entities/session.ts` | Exhaustive event-to-transcript reducer |
| `ui/src/entities/routes.ts` | The `Route` union, `routeKey`, and which routes carry a mode |
| `ui/src/features/navigation.svelte.ts` | The back/forward stack and the derived mode |
| `ui/src/features/menu.ts` | Native context menus and the single live-menu handle |
| `ui/src/features/view.ts` | Per-webview view state in `localStorage` |

Synchronous Tauri commands must take `State<T>` by value for `CommandArg` extraction; their narrow
`needless_pass_by_value` allowances reflect that framework-required boundary, not domain ownership.

Register session event listeners before calling `session_listen`; attachment events remain buffered
until then. Desktop window IDs are stable across clear/resume, while the daemon session ID can
change. Use the authoritative `session.submit` receipt for task IDs: the daemon allocates them,
regardless of the ID submitted by a client. Closing a desktop attachment never kills its daemon
session. `ApiSession` drop cancels the underlying peer.

Agent mode keeps one conversation per `(agent, external)` on the `desktop` channel, listed by
`agent.conversations` and selected with the `conversation` field on `agent.chat` and `agent.send`.
Omitting it targets `main`, which is what every pre-existing thread is. A new agent chat has no id
until its first message: the webview mints a UUID at send time and rewrites the route in place, the
same shape the code side uses for `ConversationBound`. Chat listeners are registered before opening
the stream. Unmounting or switching agents closes both chat and activity streams. Agent images are
refused; code images use `InputAttachment`.

## The webview

`ui/src` is layered `shared ← entities ← features ← widgets ← app` and imports only ever point down
that list. Layers hold files, not slices: there are no per-feature directories, no `index.ts`
barrels. `app/App.svelte` owns daemon glue and the shell; everything visual lives below it.

Navigation is a stack of plain `Route` values. Side effects happen in exactly one `$effect` in
`App.svelte`, guarded by a non-reactive `appliedKey`, so back and forward replay without reopening a
session that is already attached. **The mode is derived by walking the stack backwards** for the most
recent route that carries one — `settings`, `integrations` and `memory` carry none, so reading only
the current route strands them in the wrong mode. Never store `mode` independently.

Expanded projects are keyed by path in a separate array. `projects_add` and `projects_remove` return
the whole list, so a flag stored on a project object is wiped on the next round trip.

## Styling

Tailwind v4 through `@tailwindcss/vite`, with every design token in the `@theme` block of
`app/style.css`. That block is the palette: surfaces, the four foreground greys, and one accent used
only for the running-session spinner. Overlay colours are white-alpha rather than opaque grey so they
composite correctly on the translucent sidebar.

**Element resets belong in `@layer base`.** Author CSS outside any cascade layer beats every layer,
so an unlayered `button { background: none }` silently kills every background utility on every
button. Legacy class rules stay unlayered on purpose — they must keep winning until their component
is migrated — but element selectors must not.

One box model serves every sidebar row: 260px sidebar, containers padded `0 8px`, rows 30px tall with
`pr-1.5 pl-2.5`, a 16px icon and a 9px gap, so **every label starts at x=43** and every trailing
control occupies the same 26px box. `shared/Row.svelte` owns it.

Everything outside the sidebar is still class-based, in the `@layer components` block at the bottom
of `app/style.css`. That block is a resting state, not a design: it carries no colour of its own,
only the tokens, so each screen can be redesigned on its own without touching the others. Delete a
screen's rules in the same commit that rewrites its markup.

Icon buttons take no background on hover; only the glyph lightens. Rows are the opposite.

The traffic lights cannot be moved at runtime — `set_traffic_light_position` is not on
`WebviewWindow` and `unsafe` is forbidden — so whichever column sits at x=0 must reserve the 91px
gutter. `WindowChrome` therefore mounts in the sidebar header when expanded and in the main header
when collapsed, never both.

The main window is `transparent` with the macOS `sidebar` material, so the effect view covers the
whole window and `:root` must stay transparent. Every surface that is not the sidebar paints its own
opaque background, and the status bar starts at column 2 so it does not slice the sidebar. Do not add
CSS `backdrop-filter` on top: with nothing behind the webview there is nothing for it to blur, and
`mask-image` on a scroller in this window leaves stale pixels.

Markdown escapes source HTML before rendering and sanitizes the result. Links open only through
HTTP(S)/mailto. Keep native IPC a fixed command surface, not a general method proxy. Native drag/drop
interception is disabled so HTML file drops reach the image composer.

## Computer use

Both Accessibility and Screen Recording must be granted before advertising. Permission changes are
checked every five seconds; disconnected provider links reconnect after three seconds. The provider
connection is separate from coding attachments. `Cmd+Shift+Escape` halts the host immediately, then
withdraws its advertisement. Resume is an explicit UI action. Withdrawal can surface as
`HostGone / NotStarted` through the broker; an already-dispatched call sees the host's halted error.

There are no per-action approval gates or terminal-app exclusions. The transparent click-through
border is shown while the host is executing a call, not while a tool waits between observations.
`GOAT_LOG` filters rolling `~/.goat/logs/desktop.log.*` output; never log to the webview or terminal.

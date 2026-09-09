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
| `ui/src/lib/session.ts` | Exhaustive event-to-transcript reducer |

Synchronous Tauri commands must take `State<T>` by value for `CommandArg` extraction; their narrow
`needless_pass_by_value` allowances reflect that framework-required boundary, not domain ownership.

Register session event listeners before calling `session_listen`; attachment events remain buffered
until then. Desktop window IDs are stable across clear/resume, while the daemon session ID can
change. Use the authoritative `session.submit` receipt for task IDs: the daemon allocates them,
regardless of the ID submitted by a client. Closing a desktop attachment never kills its daemon
session. `ApiSession` drop cancels the underlying peer.

Agent mode uses the daemon's one `desktop/main` conversation per agent. New message resets only the
draft, never persisted history. Chat listeners are registered before opening the stream. Unmounting
or switching agents closes both chat and activity streams. Agent images are refused; code images
use `InputAttachment`.

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

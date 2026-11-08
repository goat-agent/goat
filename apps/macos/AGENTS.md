# macOS

Read [docs/style/swift.md](../../docs/style/swift.md) before writing Swift.

The Mac app does one thing: it lets the agent use this Mac. It lives in the menu bar, signs in as a
device, keeps the device WebSocket open, and answers the server's `computer.*` calls
(`apps/server/src/lib/computers/protocol.ts`). Conversations happen in the web app.

## Map

- `Goat/` — the app. `AppModel` connects the account to the device connection.
  - `Account/` — email-code sign-in, the session token and this install's id in the Keychain.
  - `Connection/` — `DeviceConnection` (the WebSocket and JSON-RPC), `ComputerUse` (checks the
    user's switch and the permissions, and maps the server's types to `Computer` and back), and
    `ForegroundConsent` (the question asked before an app comes to the front).
  - `Permissions/`, `MenuBar/` — the two grants and the menu bar window.
- `Computer/` — Swift package that sees and acts in windows, apart from the app so it builds and
  tests on its own. It knows nothing of the server.
- `GoatAPI/` — Swift package whose types and client are generated at build time from
  `apps/server/openapi.json`, which `Sources/GoatAPI/openapi.json` links to.

## Commands

```sh
swift format lint --strict --recursive Goat Computer/Sources Computer/Tests Computer/Package.swift \
  GoatAPI/Package.swift GoatAPI/Sources
xcodebuild -project Goat.xcodeproj -scheme Goat -destination 'platform=macOS' \
  -skipPackagePluginValidation build
(cd Computer && swift test)
```

Use the Xcode toolchain (`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`) for
`swift test`. Debug builds talk to `GOAT_SERVER_URL` (`http://localhost:3000`). Write UI text in
English; `Goat/Localizable.xcstrings` holds Korean. `xcodebuild` does not add new keys, so pass the
build's `.stringsdata` files to `xcrun xcstringstool sync` and fill in Korean before finishing.

## How `Computer` acts

Measured on macOS 26.6.2 with windows covered by another app's window (Chrome, Electron, Safari,
TextEdit, Calculator, Clock):

- The user's pointer, front app, and typing are never touched in the background. Each action
  first makes its window the app's main window through accessibility: without that, keys go to
  the app's other window and AppKit and WebKit drop a single click.
- A plain click on a native control is pressed through accessibility. Catalyst apps ignore posted
  pointer events but take the press.
- Everything else is posted to the app's process with `CGEventPostToPid`, carrying the window
  number (event field 51) and a click group (58). Neither field is in Apple's headers; without
  them a background app drops the events. No private API is used.
- Hover, pointer events in Catalyst apps, and ⌘ shortcuts in Chromium apps only work in front.
  Neither a menu item pressed through accessibility nor cua's private focus record makes a
  Chromium shortcut apply in the background. These return `needs_foreground`; with
  `foreground: true` the user is asked, the app comes forward, and the pointer and front app go
  back afterwards.
- Chromium stops drawing, and applying scrolls, while a window is fully covered. `Computer` slides
  such a window until a 40-point strip shows and puts it back after a minute of no use.
- Capturing a window triggers macOS's screen-recording prompt, which takes the front once; the
  menu bar window asks for both permissions up front.

## Distribution

The app is not sandboxed, because the accessibility of other apps does not work in the sandbox.
It ships with Developer ID and the hardened runtime, and runs as a menu bar app (`LSUIElement`).

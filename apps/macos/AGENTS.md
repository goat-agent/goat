# macOS

Read [docs/style/swift.md](../../docs/style/swift.md) before writing Swift. The window shows the
web app ([apps/web](../web/AGENTS.md)); the Swift here draws only what lives outside it, with the
parts and rules of [apps/ios/AGENTS.md](../ios/AGENTS.md).

## Map

- `Goat/` — `GoatApp` declares the main window (no title bar), the one-time permissions window,
  and the menu bar item; ⌘, opens the web app's settings. `App/RootView` fills the window with
  the web app and keeps the device socket open while someone is signed in; `App/Session` keeps
  the session token in the Keychain.
  - `Web/` — the web app inside Goat. `WebApp` owns the page, loaded once at launch and kept
    for the app's life. `AppScheme` serves `goat://app`: the web app's files, and under
    `/server` the goat server with the session token added. `RequestHandler` answers the page's
    requests.
  - `MenuBar/`, `Permissions/` — the surfaces outside the window.
  - `Borrowing/BorrowIndicator` — the ring that follows the pointer and the capsule at the top
    while the agent has the user's pointer.
- `Goat/Device/` — the link to the server. `DeviceConnection` keeps the WebSocket open and hands
  each JSON-RPC request to `ComputerService`, which checks the user's switch and the permissions,
  decodes the params into the generated `GoatAPI` types, and calls `Computer`.
- `Computer/` — Swift package that does computer use, apart from the app so it builds and tests on
  its own. It takes and returns the generated `GoatAPI` types, so the server's schemas are the
  only contract.
  - `Snapshot` reads a window's accessibility tree into the element list; `Elements` keeps the
    refs, which stay the same while their element exists.
  - `Capture` takes window and screen images with ScreenCaptureKit and scales them to at most
    1280 px on the long edge, at most one pixel per point.
  - `Computer` decides how each action runs (see Design). `Input`, `Values`, `Menus`, and
    `Opening` do the work.
  - `AX.swift` holds the one private call, `_AXUIElementGetWindow`, which ties an accessibility
    window to its window server id.
- `Bridge/` — Swift package with the types of what the page and the app say to each other,
  generated at build time from `openapi.json`, which apps/web exports from its contract
  (`bun run bridge` there); never edit it by hand. `Messages.swift` decodes requests and writes
  the scripts that put the host and events on the page.
- `../shared/GoatKit/UI`, `../shared/GoatKit/Server` — the design system's parts and this
  install's identity, shared with iOS; `Goat/Localizable.xcstrings` holds this app's strings.
  `../shared/GoatAPI/` — the server's client; see the iOS notes.

## Commands

```sh
swift format lint --strict --recursive Goat Computer/Sources Computer/Tests Computer/Package.swift \
  Bridge/Package.swift Bridge/Sources
xcodebuild -project Goat.xcodeproj -scheme Goat -destination 'platform=macOS' \
  -skipPackagePluginValidation build
(cd Computer && swift test)
(cd Computer && GOAT_LIVE_COMPUTER=1 swift test --filter LiveTests)          # reads the screen
(cd Computer && GOAT_LIVE_ACTIONS=1 swift test --filter LiveActionTests)     # opens Calculator
```

Use the Xcode toolchain (`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`) for
`swift test`. The live tests need Accessibility and Screen Recording for the terminal running
them. Debug builds talk to `GOAT_SERVER_URL` (`http://localhost:3000`) and load the page from
the web dev server at `GOAT_WEB_URL` (`http://localhost:5173`), so run `bun run dev` first; a
saved web change shows in the window at once. Release builds copy `apps/web/dist` into the app,
so build the web app first.

## Previews

Render previews as [apps/ios/AGENTS.md](../ios/AGENTS.md) describes, with this folder's
`Goat.xcodeproj` open instead of the iPhone's. The Mac draws each preview inside a window and
offers only static renders, in light or dark and in English or Korean.

## Design

- The page runs on `goat://app`, one origin for its files and the server, so it needs no cookies
  and the server no CORS. The page never holds the session token: `AppScheme` adds it to
  requests under `/server`, keeps it from the sign-in response, and forgets it on sign-out.
  Responses stream through as they arrive, so the server's event streams work unchanged.
- The page has no title bar above it; its bars ask the app to move the window
  (`performDrag`), and the window's buttons sit on its sidebar. Links that leave the web app
  open in the default browser; a service's sign-in opens in the system's sign-in sheet.
- Every request the page makes is a case of the contract's `RequestMethod`, so a new one does
  not compile until `RequestHandler` answers it.
- The agent may use this Mac only while the user's switch is on and both permissions are
  granted; `computer.status` reports that, and every other call fails without it.
- Work happens in the background whenever accessibility can do it: pressing, selecting, showing
  context menus, setting values, inserting text at the caret, choosing menu bar items, keys sent
  to one app, and opening apps without activating them. The user's pointer and front app stay
  put.
- Pointer gestures (point clicks, double and modified clicks, hover, drag, scroll) need the real
  pointer, so `Computer` borrows the screen: it brings the app forward through accessibility
  (since macOS 14 a background app cannot activate another), acts, and puts the pointer and the
  front app back. Hover looks before giving the pointer back, since what it reveals goes away.
- While borrowing, `BorrowIndicator` shows the ring and capsule; Esc ends the borrow. Stop in the
  menu bar also switches computer use off.
- Goat's own windows are never offered to the agent, so it cannot switch off its own limits.
- Apps build accessibility trees on demand: Chromium and Electron apps get the flags that turn
  theirs on, and a window that reads empty is read again for up to a second.
- The app is not sandboxed, because accessibility of other apps does not work in the sandbox;
  it ships with Developer ID and the hardened runtime. Release builds register as a login item,
  so the Mac is reachable after a restart.

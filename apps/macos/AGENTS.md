# macOS

Read [docs/style/swift.md](../../docs/style/swift.md) before writing Swift. Design rules,
localization, and API values work as in [apps/ios/AGENTS.md](../ios/AGENTS.md); the screens in
`../shared/GoatKit` are the same code.

## Map

- `Goat/` — the Mac's screens. `GoatApp` declares the main window (no title bar), the one-time
  permissions window, Settings, and the menu bar item. `App/RootView` connects the device socket
  once someone is signed in.
  - `Onboarding/` — sign-in and the first agent, one small card per step.
  - `Home/` — an inset sidebar as shadcn/ui draws it: the window is the sidebar's color, the
    sidebar sits on it with the window's buttons, and the conversation is a raised card. Dragging
    the card's edge resizes the sidebar; narrow, it becomes a column of faces.
  - `Conversation/` — the conversation card and its composer, built from the shared bubbles.
  - `MenuBar/`, `Permissions/`, `Settings/` — the other surfaces.
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
- `../shared/GoatKit/`, `../shared/GoatAPI/` — shared with iOS; see the iOS notes.

## Commands

```sh
swift format lint --strict --recursive Goat Computer/Sources Computer/Tests Computer/Package.swift
xcodebuild -project Goat.xcodeproj -scheme Goat -destination 'platform=macOS' \
  -skipPackagePluginValidation build
(cd Computer && swift test)
(cd Computer && GOAT_LIVE_COMPUTER=1 swift test --filter LiveTests)          # reads the screen
(cd Computer && GOAT_LIVE_ACTIONS=1 swift test --filter LiveActionTests)     # opens Calculator
```

Use the Xcode toolchain (`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`) for
`swift test`. The live tests need Accessibility and Screen Recording for the terminal running
them. Debug builds talk to `GOAT_SERVER_URL` (`http://localhost:3000`).

## Design

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

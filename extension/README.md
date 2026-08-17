# goat Chrome extension

Two halves:

- `background.js` is a service worker. It owns the native-messaging port, so the
  port survives the side panel being closed. It is a relay, not a browser
  vocabulary: it forwards CDP commands to `chrome.debugger`, answers the five tab
  operations from `chrome.tabs`, and pushes `chrome.debugger.onEvent` back up the
  port. Every decision about what to send lives in Rust.
- `panel.html` / `panel.js` are the side panel, which shrinks the page and puts
  goat beside it. **The panel never opens its own native port.** It asks
  `background.js` over `chrome.runtime` and renders what it is told. Two ports
  would advertise `host.browser` twice from one browser, and the daemon's lease
  keys on device, instance and boot epoch, so it would treat them as two
  different browsers.

## The attachment is session-scoped

`chrome.debugger` stays attached to the tab goat is driving until the session
ends or a `detach` command arrives. Attaching per action would drop every CDP
event between actions, and the Rust side listens for them — dialogs are answered
from `Page.javascriptDialogOpening` and navigation settles on
`Page.loadEventFired`. The visible cost is that Chrome's "is debugging this
browser" banner stays up for the whole session; that banner is the human's signal
that goat holds the tab.

## Why `chrome.debugger` and not content scripts

`chrome.debugger.sendCommand` is the Chrome DevTools Protocol, the same wire the
Rust `Cdp` type speaks. Keeping the extension at that layer is what makes the
browser vocabulary single-source: actions, snapshots, ref lifetimes and
navigation settling are decided once, in Rust, and only commands cross. A
content-script implementation would be a strict subset and would fork the tool
schema.

## Why an extension at all

Chrome 136 stopped honouring `--remote-debugging-port` against the default user
data directory. An extension runs *inside* that profile, so it is the only way to
drive the browser the human is already signed into — which is the point: goat
acts under the human's own login, in the window the human is watching.

## The extension ID is pinned

`manifest.json` carries a `key`, so Chrome derives the same ID —
`mnpgokpnlppedkmpglalagjjbfidbcdf` — wherever the unpacked extension is loaded
from. Without it the ID comes from the directory path, and every machine would
need its own hand-edited `allowed_origins`. The private half of that key is not
in this repository and is not needed: it only signs a `.crx`, and goat installs
unpacked.

## Install

1. Build and install the host: `cargo build --release` then link
   `target/release/goat` somewhere stable. The wrapper named in
   `com.goat.browser_host.json` must exec `goat browser-host`.
2. Copy `com.goat.browser_host.json` into Chrome's native-messaging hosts
   directory for your platform. Its `allowed_origins` already names the pinned
   ID.
3. Load `extension/` as an unpacked extension.

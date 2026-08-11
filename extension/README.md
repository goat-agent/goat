# goat Chrome extension

Two halves:

- `background.js` is a service worker. It owns the native-messaging port, so the
  port survives the side panel being closed. It attaches `chrome.debugger` only
  for the duration of one action and detaches afterwards, which keeps Chrome's
  "is debugging this browser" banner scoped to actual agent activity.
- `panel.html` / `panel.js` are the side panel, which shrinks the page and puts
  goat beside it.

## Why `chrome.debugger` and not content scripts

`chrome.debugger.sendCommand` is the Chrome DevTools Protocol. Using it keeps the
tool surface identical to the daemon's own CDP browser backend, so the same
`host.browser` actions work whichever backend serves them. A content-script
implementation would be a strict subset and would fork the tool schema.

## Why an extension at all

Chrome 136 stopped honouring `--remote-debugging-port` against the default user
data directory. An extension runs *inside* that profile, so it is the only way to
drive the browser the human is already signed into.

## Install

1. Build and install the host: `cargo build --release` then link
   `target/release/goat` somewhere stable. The wrapper named in
   `com.goat.browser_host.json` must exec `goat browser-host`.
2. Load `extension/` as an unpacked extension and copy its ID.
3. Put the ID into `allowed_origins` as `chrome-extension://<id>/`.
4. Copy `com.goat.browser_host.json` into Chrome's native-messaging hosts
   directory for your platform.

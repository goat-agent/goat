# AGENTS.md — goat-capability

The daemon-side broker for capabilities living on the human's machine: `host.browser` today,
`host.computer` next.

## Routing is a lease, not a pin

The lease is keyed by device, provider instance and boot epoch.

| Event | Result |
|---|---|
| provider disconnects | the lease pauses; the session survives |
| same instance returns with a new boot epoch (a different browser profile) | explicit rebind required |

Never fail a side-effecting call over to another machine. Every error carries an execution
disposition — `NotStarted`, `KnownFailed`, `OutcomeUnknown` — so a model is told whether retrying is
safe.

Human decisions do not go through this path. An answer is a durable resource settled by a
compare-and-set, never a connection-scoped reverse call.

## One browser is one connection

`extension/background.js` owns the single native-messaging port. The side panel asks it over
`chrome.runtime` rather than opening a second port, because two ports would advertise `host.browser`
twice and the lease would read them as two browsers.

Riding that port is free: `open_serving` is already bidirectional, so the panel's calls are the
outward direction of the connection whose inward direction serves `host.browser`.

**Keep the panel a closed set of verbs, never a method proxy.** `goat-browser-host`'s `panel` module
accepts `panel.open`, `panel.submit`, `panel.interrupt` and `panel.answer`, and nothing else. That
connection is local, so it carries `Grant::Admin`. A relay forwarding whatever method arrived would
put `admin.credential_set` one message away from a surface that renders pages the human did not write.

## Desktop control belongs to a signed client app

`host.computer` is provided by a signed `.app`, never by this binary. A `goat-tool-computer` once
drove the desktop in-process with synthetic input and pixel coordinates. It was deleted because three
things were wrong at once, none fixable here:

| Problem | Consequence |
|---|---|
| the `goat` binary is ad-hoc signed (`TeamIdentifier=not set`) | macOS keys Accessibility and Screen Recording grants to the code signature, so every rebuild forced a re-grant |
| `DesktopBackend` acted on the *daemon* host | the wrong machine for `goat code --remote` |
| driving Terminal.app sidesteps `goat-sandbox`'s Seatbelt profile | the shell sandbox is voided |

A signed `.app` has a stable identity and holds its grants across updates.

The contract is **element tree, refs and named actions**, with screenshots as a secondary channel.
Not pixel coordinates. The provider must refuse to automate terminal apps, which is what keeps the
shell sandbox meaningful.

This split differs from `host.browser`. CDP is already a wire protocol, so the browser vocabulary
stays in Rust and only commands cross. `AXUIElement` handles are process-local opaque pointers, so
tree walking and ref minting happen inside the provider.

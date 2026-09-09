# AGENTS.md — goat-computer-host

The provider-side `host.computer` implementation. Only the signed desktop app hosts native desktop
control; the daemon relays commands to its capability lease. The fake provider is for isolated
broker tests, never a production fallback.

Wire coordinates are logical display points with a top-left origin. The model-facing computer tool
uses pixels of its latest screenshot and maps them through the returned `display` rectangle. Zoom
captures full resolution before cropping and returns the actual crop bounds, including pixel-edge
rounding. PNGs preserve aspect ratio and never upscale beyond their captured width.

AX references belong to the latest successful snapshot only, across all holders of this host.
Snapshots mint process-monotonic `sN` IDs and bounded `eN` references. A stale snapshot ID is rejected
before any event is posted. Native AX handles never leave this process. Walks are bounded to depth
24 and 400 visited nodes; AX child ranges are clamped to actual child counts.

`ComputerHost` admits one call at a time. Its busy guard lives in the blocking operation, so dropping
an async caller cannot clear busy while native input is still running. Halt is checked again before
the blocking operation starts. Native errors distinguish `NotStarted` from `OutcomeUnknown`;
never retry uncertain side effects automatically. Chords validate every token before pressing keys,
and held input is released on errors and drop.

`macos/control` owns one native worker thread. `macos/capture` uses ScreenCaptureKit 9 and PNG
encoding. Version 10 pulls apple-cf 0.10 alongside axuielement 0.9's apple-cf 0.9, creating duplicate
Swift symbols; keep those bridge versions compatible. The build script discovers Swift runtime
library paths with `xcrun`; workspace macOS linker flags provide `/usr/lib/swift` at runtime.

`goat-computer-host-apps` wraps app enumeration/focus and Quartz mouse events. Quartz coordinates
avoid Enigo's cursor conversion mixing AppKit points with display pixels. Enigo handles keyboard,
text and scrolling, initialized lazily after permission checks. All these crates inherit the
workspace unsafe-code prohibition; no new FFI exception is needed with the selected safe APIs.

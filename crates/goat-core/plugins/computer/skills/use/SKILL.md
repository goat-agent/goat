---
description: How to see and use apps on this computer with mcp.computer. Load it before the first computer call in a session.
user-invocable: false
---
Prefer an app's own MCP server or plugin when there is one; use the computer when nothing else reaches the app.

Look before you act:
1. `list_windows()` shows the apps and window ids.
2. `list_elements({ window, find })` lists the elements of a window with refs. Filter in JavaScript and return only what you need.
3. `screenshot({ window })` when the elements are thin, such as canvases, games or some web views.

Act by ref when you can: `click`, `type` and `scroll` with a `ref` use the element itself and do not move the pointer. Use `x` and `y` with the `shot` they came from only when there is no ref.

After each step, check `changed`. When it is false, look again with list_elements or a screenshot before retrying, and try a position only when the ref did nothing.

Refs last until list_elements runs again for that window. A `stale_ref` error means list again; never reuse an old ref.

`type` with a ref sets a field's value; without one it types into the focus. `press` takes shortcuts such as "cmd+s".

The user approves each app once. When the user refuses, stop and say what you needed. Never try to control the terminal goat runs in, and never type into a password field.

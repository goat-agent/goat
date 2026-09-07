# AGENTS.md — goat-tool-browser

**This crate launches nothing.** It is the browser vocabulary — actions, snapshots and the
`data-goat-ref` lifecycle, navigation settling, dialogs, network and console observation — over a
`Transport` trait. That trait carries one typed `BrowserCommand`, a CDP command defined in
`goat-api`, and returns its result.

| Implementation | Reaches |
|---|---|
| production | the human's Chrome, through `host.browser`. See `crates/goat-capability/AGENTS.md`. |
| `transport::fake` | nothing, which is what makes the vocabulary testable without a browser |

No test in the repository drives a real Chrome. `transport::fake` is the coverage, and the
extension's end of the pipe is uncovered.

## Registration bypasses `ToolRegistry::builtin()`

`CodingEngine::new` takes a `browser: Option<Arc<dyn Transport>>` and pushes the tool only when one is
present. The gate is "a capability provider is attached", not a config flag.

The agent reaches the same browser through `goat_agent_tool_browser::register`, which needs the
`CodeSessionHub` and is therefore wired explicitly in `goat-runtime`.

## Not a second backend

`goat-tool-web` and `goat-search-provider-duckduckgo` do launch a throwaway headless Chrome through
`chromiumoxide`. They fetch anonymously and never touch the human's session, so they are a different
concern, not an alternative transport.

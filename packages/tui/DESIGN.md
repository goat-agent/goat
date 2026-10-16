# @goat/tui

The terminal screen of goat, drawn by its own renderer on Bun built-ins. `@goat/terminal` owns the terminal device. This package owns everything the user sees. `apps/cli` connects them to the engine.

This document records the models that are hard to change later and where each future feature goes. Code is written against it.

## Goals

1. **Light.** The floor, measured on 2026-10-07 with Bun 1.4.2 (2,000 messages, 120x40, 200 deltas per second for 15 s): 25 MiB idle, 41 MiB peak, 0.25 s CPU. The same scenario cost Ink 388 MiB and 12.9 s. Every feature must keep the screen near the floor: render only visible lines, write only changed lines, draw at most one frame per 16 ms.
2. **Clean.** The default screen is the transcript and the composer. Nothing else appears until it has something to show.
3. **Extensible.** Every planned feature has a place in the models below. Adding a feature adds a registration, not a model.
4. **One session, many screens.** The screen is a client of the engine protocol. Data that every screen must show comes from the engine. Code in this package only draws and handles local interaction.

Not planned: inline mode, a default status line, a working indicator, animation, approval prompts. goat always runs with full access and has no permission modes.

## Packages

| Package          | Owns                                                                                        | Does not know                |
| ---------------- | ------------------------------------------------------------------------------------------- | ---------------------------- |
| `@goat/terminal` | Terminal modes, input parsing, capability detection, frame writing, cursor, restore on exit | goat, sessions, providers    |
| `@goat/tui`      | Screen state, layout, blocks, composer, dock, overlays, commands, keymap, theme             | How bytes reach the terminal |

Widgets such as the editor and the list belong here, not in the terminal package, so the terminal package stays small. Text width, wrapping, slicing and markdown use `Bun.stringWidth`, `Bun.wrapAnsi`, `Bun.sliceAnsi` and `Bun.markdown`.

## Models

### Screen state

Screen state has two parts, and rendering reads both.

- **Session state** comes from protocol events through a reducer: transcript blocks, pending requests, and status values. Every screen derives the same session state.
- **Local state** belongs to this screen: scroll position, focus, expanded blocks, selection, the composer document, and open overlays. It is never sent to the engine.

Input changes local state or sends a command to the engine. It never edits session state directly.

### Block

The transcript is an ordered list of blocks. Each block has a stable `id`, a `version` that increases on every change, and a `kind`: user, agent text, reasoning, tool call, notice.

A tool result carries display data from the tool, as a display kind and its data: terminal output, diff, file list, markdown. The screen registers one renderer per block kind and per display kind. An unknown display kind falls back to the result's text parts.

```ts
type Renderer<T> = (value: T, width: number, context: RenderContext) => readonly Line[];
```

`RenderContext` holds the theme, the terminal capabilities, and the local state of that block, such as whether it is expanded. Results are cached by block id, version, width, theme, and local state. While a message streams, only its block renders again.

### Line

```ts
interface Line {
  readonly text: string;
  readonly copy?: { readonly column: number; readonly text: string };
  readonly images?: readonly ImagePlacement[];
}
```

- `text` is ANSI and fits the width exactly. Bun's text built-ins and frame diffing work on strings.
- `copy` is the plain text that selection copies and the column where it starts. Borders, gutters and markers are not copied.
- `images` places pictures for graphics protocols that draw at a position outside the text. Kitty Unicode placeholders need no placement because they are text.

### Composer document

The composer edits a list of segments: text and tokens. Token kinds are pasted text, image, file, and mention. A token is atomic: the cursor moves over it and deletion removes it whole. It shows as plain text in brackets, such as `[Pasted text #1 +42 lines]` or `[Image #1]`, in the dim role.

On submit, pasted text expands to its content, an image becomes a `MediaPart`, and the document becomes one user message. Undo works on segments.

### Layers and focus

Input goes through a stack of layers: overlays, then dock items, then the composer, then the transcript. The focused layer gets the key first. A key it does not handle goes down the stack, and then to the keymap.

### Commands and keymap

A command has an id, a title, a context in which it applies, and a run function. Engine commands arrive through the protocol. Screen commands are registered here. The command list shows both.

The keymap maps keys to command ids for each context, and the user's settings can override it. A slash command is a command with a slash name. Built-in features register through the same API that extensions will use. The API stays internal until it is stable.

### Capabilities

At start, the terminal package sends its queries followed by a primary device attributes request (DA1). Every terminal answers DA1, so a query still unanswered when the DA1 reply arrives is unsupported, and start-up does not wait on a timer. A timer applies only if DA1 never answers. The queries cover the kitty keyboard protocol, synchronized output, graphics (kitty, iTerm2, sixel), OSC 52 clipboard, OSC 8 links, color depth, and the background color. Renderers read the result, and every feature has a fallback for a terminal that does not support it.

### Theme

Code uses roles, never colors: text, dim, accent, error, warning, success, added, removed. The default theme maps roles to the 16 ANSI colors, so the user's terminal theme decides the look. A theme with exact colors is optional.

## Where features go

| Feature                                                                                | Home                                                                                                                                                                 |
| -------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Markdown, code blocks, tables                                                          | `Bun.markdown.ansi`: wrapping, syntax highlighting and OSC 8 links are built in. A role-based renderer on `Bun.markdown.render` replaces it only if theming needs it |
| Reasoning, folded by default                                                           | Block kind and expanded flag in local state                                                                                                                          |
| Tool output, diffs, file lists, search results                                         | Display kinds and their renderers                                                                                                                                    |
| Image preview                                                                          | `Line.images` or kitty placeholders, chosen by capability. Fallback `[Image #1 1024x768]`                                                                            |
| Links                                                                                  | OSC 8 in `Line.text` when supported                                                                                                                                  |
| Questions from the model, todo, queued messages, background tasks                      | Dock items from session state                                                                                                                                        |
| Pickers for model, session (every device), file, command; search; `/context`; help     | Overlays that share one searchable list                                                                                                                              |
| Paste tokens, images, `@` mentions, `/` commands, `!` shell, vim mode, external editor | Composer document and composer commands                                                                                                                              |
| Status items                                                                           | Status registration. None by default                                                                                                                                 |
| Subagents                                                                              | A block kind that opens its session in a stacked view                                                                                                                |
| A session on another device needs input                                                | Engine event, shown as a dock item or a terminal notification                                                                                                        |
| Selection and copy                                                                     | Local selection over `Line.copy`, copied with OSC 52                                                                                                                 |
| Themes, keybindings                                                                    | Settings, validated with Zod                                                                                                                                         |

## Frame

1. A protocol event or an input changes state and marks the screen dirty.
2. At most once per 16 ms, layout gives heights to the transcript, the dock, and the composer. The composer grows with its content up to a third of the screen. Overlays float.
3. Visible blocks render, mostly from the cache.
4. The frame joins transcript lines, dock, composer, and overlays. Overlays are spliced into lines with `Bun.sliceAnsi`.
5. The frame is compared with the previous frame line by line. Changed lines are written in one synchronized update.
6. The real cursor moves to the composer caret, so IME composition appears in place. It is hidden when no input has focus.

## Terminal

| Mode                    | Reason                                                                |
| ----------------------- | --------------------------------------------------------------------- |
| Raw input               | Keys arrive at once                                                   |
| Alternate screen (1049) | Reattach redraws from state; any line can change                      |
| Bracketed paste (2004)  | Pasted newlines do not submit                                         |
| Kitty keyboard protocol | Shift+Enter and Esc are unambiguous. Legacy parsing where unsupported |
| SGR mouse (1006)        | Wheel scroll. Option-drag or Shift-drag still selects natively        |

- **Input events:** key (name, modifiers, text), paste (text), and mouse (kind, position). UTF-8 split across chunks is joined. Unknown sequences are dropped. Without the kitty protocol, a lone ESC is the Esc key when no byte follows within a short time.
- **Keys:** Enter submits. Shift+Enter and Ctrl+J insert a newline. Ctrl+J is LF, not CR, so it works in every terminal without setup.
- **Stop and exit:** Esc stops the running turn. Ctrl+C stops the running turn, or clears a non-empty composer, or exits when pressed twice on an empty composer. Ctrl+D exits on an empty composer. Transient hints, such as "Press Ctrl+C again to exit", replace the composer placeholder instead of taking a line.
- **Scroll:** the mouse wheel and PgUp/PgDn scroll the transcript. The view follows new output while it is at the bottom and stops following when the user scrolls up.
- **Selection until in-app selection exists:** mouse reporting turns off plain drag selection, so Option-drag (macOS) or Shift-drag selects natively.
- **Paste:** a paste longer than 10 lines becomes a pasted-text token. Ten lines is about the composer's maximum height on a 40-row terminal, so a longer paste could not be read in place anyway.
- **Resize:** a resize redraws the whole frame.
- **Exit:** normal exit, an error, or a signal turns every mode off and leaves the alternate screen.

## Testing

- Renderers and layout are pure. Tests compare `Line[]`.
- The frame writer is tested against a small fake screen that applies cursor moves and text.
- The input parser is tested with byte sequences, including sequences split across chunks.
- Recorded protocol events replay into frames.
- The benchmark scenario runs with budgets, so a feature that leaves the floor fails the build.

## Open decisions

- How images enter the composer: a key that reads the clipboard, a dropped file path, or both.
- Keyboard navigation inside the transcript.
- How much of vim mode to support.

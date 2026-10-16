import type { Key } from "@goat/terminal";

export type Command =
  | "submit"
  | "newline"
  | "stop"
  | "interrupt"
  | "exit-if-empty"
  | "delete-back"
  | "delete-forward"
  | "delete-word-back"
  | "delete-line-start"
  | "delete-line-end"
  | "left"
  | "right"
  | "word-left"
  | "word-right"
  | "up"
  | "down"
  | "line-start"
  | "line-end"
  | "page-up"
  | "page-down";

const bindings: ReadonlyMap<string, Command> = new Map([
  ["enter", "submit"],
  ["shift+enter", "newline"],
  ["alt+enter", "newline"],
  ["ctrl+j", "newline"],
  ["escape", "stop"],
  ["ctrl+c", "interrupt"],
  ["ctrl+d", "exit-if-empty"],
  ["backspace", "delete-back"],
  ["ctrl+h", "delete-back"],
  ["delete", "delete-forward"],
  ["alt+backspace", "delete-word-back"],
  ["ctrl+w", "delete-word-back"],
  ["ctrl+u", "delete-line-start"],
  ["ctrl+k", "delete-line-end"],
  ["left", "left"],
  ["right", "right"],
  ["alt+left", "word-left"],
  ["ctrl+left", "word-left"],
  ["alt+b", "word-left"],
  ["alt+right", "word-right"],
  ["ctrl+right", "word-right"],
  ["alt+f", "word-right"],
  ["up", "up"],
  ["down", "down"],
  ["home", "line-start"],
  ["ctrl+a", "line-start"],
  ["end", "line-end"],
  ["ctrl+e", "line-end"],
  ["pageup", "page-up"],
  ["pagedown", "page-down"],
]);

export function commandFor(key: Key): Command | undefined {
  const modifiers = [
    key.ctrl ? "ctrl+" : "",
    key.alt ? "alt+" : "",
    key.shift ? "shift+" : "",
    key.meta ? "meta+" : "",
  ].join("");
  return bindings.get(`${modifiers}${key.name}`);
}

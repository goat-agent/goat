import { useSyncExternalStore } from "react";

/**
 * Material's window size classes. Compact shows one pane at a time, medium shrinks the agent list
 * to a column of faces, expanded shows the list beside the conversation. The CSS breakpoints
 * `medium:` and `expanded:` in styles.css use the same widths.
 */
export type WindowClass = "compact" | "medium" | "expanded";

const medium = matchMedia("(min-width: 600px)");
const expanded = matchMedia("(min-width: 840px)");

function current(): WindowClass {
  if (expanded.matches) return "expanded";
  return medium.matches ? "medium" : "compact";
}

function subscribe(listener: () => void): () => void {
  medium.addEventListener("change", listener);
  expanded.addEventListener("change", listener);
  return () => {
    medium.removeEventListener("change", listener);
    expanded.removeEventListener("change", listener);
  };
}

export function useWindowClass(): WindowClass {
  return useSyncExternalStore(subscribe, current);
}

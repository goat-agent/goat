import { useSyncExternalStore } from "react";

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

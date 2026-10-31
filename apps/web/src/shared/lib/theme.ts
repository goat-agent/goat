import { createStore } from "./store";

export type Theme = "system" | "light" | "dark";

// The theme belongs to the browser, not the account: one account is used from devices that want
// different themes. index.html applies the same rule before the first paint.
const KEY = "theme";
const systemDark = matchMedia("(prefers-color-scheme: dark)");

function saved(): Theme {
  const value = localStorage.getItem(KEY);
  return value === "light" || value === "dark" ? value : "system";
}

function apply(theme: Theme): void {
  const dark = theme === "dark" || (theme === "system" && systemDark.matches);
  document.documentElement.classList.toggle("dark", dark);
}

const theme = createStore<Theme>(saved());

systemDark.addEventListener("change", () => {
  apply(theme.get());
});

export function setTheme(next: Theme): void {
  if (next === "system") localStorage.removeItem(KEY);
  else localStorage.setItem(KEY, next);
  apply(next);
  theme.set(next);
}

export const useTheme = theme.use;

export function isTheme(value: string): value is Theme {
  return value === "system" || value === "light" || value === "dark";
}

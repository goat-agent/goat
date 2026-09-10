import type { Route } from '../entities/routes';

const KEY = 'goat.desktop.view.v1';

export interface ViewState {
  collapsed: boolean;
  expanded: string[];
  route: Route | null;
}

const EMPTY: ViewState = { collapsed: false, expanded: [], route: null };

const isRoute = (value: unknown): value is Route =>
  !!value && typeof value === 'object' && typeof (value as Route).kind === 'string';

export function loadView(): ViewState {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return EMPTY;
    const parsed = JSON.parse(raw) as Partial<ViewState>;
    return {
      collapsed: parsed.collapsed === true,
      expanded: Array.isArray(parsed.expanded) ? parsed.expanded.filter(item => typeof item === 'string') : [],
      route: isRoute(parsed.route) ? parsed.route : null,
    };
  } catch {
    return EMPTY;
  }
}

let timer: ReturnType<typeof setTimeout> | undefined;

export function saveView(state: ViewState): void {
  clearTimeout(timer);
  timer = setTimeout(() => {
    try {
      localStorage.setItem(KEY, JSON.stringify(state));
    } catch {
      return;
    }
  }, 250);
}

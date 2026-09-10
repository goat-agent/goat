import { modeOf, routeKey, type Mode, type Route } from '../entities/routes';

const LIMIT = 100;

export class Navigation {
  #entries = $state<Route[]>([{ kind: 'welcome' }]);
  #index = $state(0);

  get current(): Route { return this.#entries[this.#index]; }
  get canBack(): boolean { return this.#index > 0; }
  get canForward(): boolean { return this.#index < this.#entries.length - 1; }
  get entries(): readonly Route[] { return this.#entries; }

  get mode(): Mode {
    for (let i = this.#index; i >= 0; i--) {
      const found = modeOf(this.#entries[i]);
      if (found) return found;
    }
    for (let i = this.#index + 1; i < this.#entries.length; i++) {
      const found = modeOf(this.#entries[i]);
      if (found) return found;
    }
    return 'code';
  }

  latest(mode: Mode): Route | null {
    for (let i = this.#index; i >= 0; i--) {
      if (modeOf(this.#entries[i]) === mode) return this.#entries[i];
    }
    return null;
  }

  go(route: Route): void {
    if (routeKey(route) === routeKey(this.current)) return;
    const next = [...this.#entries.slice(0, this.#index + 1), route];
    this.#entries = next.length > LIMIT ? next.slice(next.length - LIMIT) : next;
    this.#index = this.#entries.length - 1;
  }

  replace(route: Route): void {
    this.#entries[this.#index] = route;
  }

  back(): void { if (this.canBack) this.#index -= 1; }
  forward(): void { if (this.canForward) this.#index += 1; }

  prune(dead: (route: Route) => boolean, fallback: Route): void {
    const current = this.current;
    const kept = this.#entries.filter(entry => !dead(entry));
    if (!kept.length) {
      this.#entries = [fallback];
      this.#index = 0;
      return;
    }
    this.#entries = kept;
    const at = kept.findIndex(entry => routeKey(entry) === routeKey(current));
    this.#index = at >= 0 ? at : kept.length - 1;
    if (dead(this.current)) {
      this.#entries = [...kept, fallback];
      this.#index = this.#entries.length - 1;
    }
  }

  get index(): number { return this.#index; }

  restore(entries: Route[], index: number): void {
    if (!entries.length) return;
    this.#entries = entries.slice(-LIMIT);
    this.#index = Math.max(0, Math.min(index, this.#entries.length - 1));
  }
}

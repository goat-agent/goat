import { useSyncExternalStore } from "react";

/** A value that lives outside React and that components read with `use`. */
export type Store<T> = {
  get: () => T;
  set: (value: T) => void;
  update: (change: (value: T) => T) => void;
  use: () => T;
};

export function createStore<T>(initial: T): Store<T> {
  let value = initial;
  const listeners = new Set<() => void>();
  const subscribe = (listener: () => void) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };
  const get = () => value;
  const set = (next: T) => {
    value = next;
    for (const listener of listeners) listener();
  };
  return {
    get,
    set,
    update: (change) => set(change(value)),
    use: () => useSyncExternalStore(subscribe, get),
  };
}

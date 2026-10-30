import { sql } from "drizzle-orm";
import { client, type Transaction } from "../db";

const CHANNEL = "agent_events";

type Listener = () => void;
/** `undefined` means any agent may have changed. */
type AnyListener = (agentId: string | undefined) => void;

const listeners = new Map<string, Set<Listener>>();
const anyListeners = new Set<AnyListener>();
let listening: Promise<unknown> | undefined;

function wake(agentId: string): void {
  for (const listener of listeners.get(agentId) ?? []) listener();
  for (const listener of anyListeners) listener(agentId);
}

function wakeAll(): void {
  for (const agentId of listeners.keys()) {
    for (const listener of listeners.get(agentId) ?? []) listener();
  }
  for (const listener of anyListeners) listener(undefined);
}

function listen(): Promise<unknown> {
  // onlisten also runs after a reconnect, when notifications may have been missed.
  return client.listen(CHANNEL, wake, wakeAll).catch((error: unknown) => {
    listening = undefined;
    throw error;
  });
}

export async function notify(tx: Transaction, agentId: string): Promise<void> {
  await tx.execute(sql`select pg_notify(${CHANNEL}, ${agentId})`);
}

export async function subscribe(agentId: string, listener: Listener): Promise<() => void> {
  listening ??= listen();
  await listening;

  const set = listeners.get(agentId) ?? new Set();
  set.add(listener);
  listeners.set(agentId, set);

  return () => {
    set.delete(listener);
    if (set.size === 0) listeners.delete(agentId);
  };
}

/** Hears every agent's changes; the listener filters out agents that are not its user's. */
export async function subscribeAll(listener: AnyListener): Promise<() => void> {
  listening ??= listen();
  await listening;

  anyListeners.add(listener);
  return () => {
    anyListeners.delete(listener);
  };
}

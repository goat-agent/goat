import type { Message, SentFile } from "@/shared/api/client";
import { createStore } from "@/shared/lib/store";

export type Pending = {
  localId: string;
  text: string;
  files: SentFile[];
  /** The newest message id when it was sent; only a later message can be its copy. */
  after: number;
  state: "sending" | "sent" | "failed";
};

const pending = createStore<ReadonlyMap<string, readonly Pending[]>>(new Map());
const NONE: readonly Pending[] = [];

function change(agentId: string, edit: (list: readonly Pending[]) => readonly Pending[]): void {
  pending.update((map) => new Map(map).set(agentId, edit(map.get(agentId) ?? NONE)));
}

export function addPending(agentId: string, item: Pending): void {
  change(agentId, (list) => [...list, item]);
}

export function setPendingState(agentId: string, localId: string, state: Pending["state"]): void {
  change(agentId, (list) => {
    const index = list.findIndex((item) => item.localId === localId);
    const item = list[index];
    return item ? list.with(index, { ...item, state }) : list;
  });
}

export function prunePending(agentId: string, messages: Message[]): void {
  const list = pending.get().get(agentId) ?? NONE;
  const still = stillPending(list, messages);
  if (still.length < list.length) change(agentId, () => still);
}

export function usePending(agentId: string): readonly Pending[] {
  return pending.use().get(agentId) ?? NONE;
}

export function stillPending(list: readonly Pending[], messages: Message[]): Pending[] {
  const copies = messages.filter((message) => message.role === "user");
  const used = new Set<number>();
  return list.filter((item) => {
    const copy = copies.find(
      (message) => !used.has(message.id) && message.id > item.after && message.text === item.text,
    );
    if (!copy) return true;
    used.add(copy.id);
    return false;
  });
}

import { LogicalPosition } from '@tauri-apps/api/dpi';
import { Menu } from '@tauri-apps/api/menu';

export type MenuEntry =
  | { separator: true }
  | { text: string; enabled?: boolean; action?: () => void };

let live: Menu | null = null;

export async function popupMenu(entries: MenuEntry[], anchor?: HTMLElement | null): Promise<void> {
  const previous = live;
  live = null;
  if (previous) await previous.close().catch(() => undefined);

  const menu = await Menu.new({
    items: entries.map((entry, index) =>
      'separator' in entry
        ? { item: 'Separator' as const }
        : {
            id: `entry-${index}`,
            text: entry.text,
            enabled: entry.enabled ?? true,
            action: () => entry.action?.(),
          }),
  });
  live = menu;

  if (anchor) {
    const rect = anchor.getBoundingClientRect();
    await menu.popup(new LogicalPosition(Math.round(rect.left), Math.round(rect.bottom + 4)));
  } else {
    await menu.popup();
  }
}

export async function dismissMenu(): Promise<void> {
  const previous = live;
  live = null;
  if (previous) await previous.close().catch(() => undefined);
}

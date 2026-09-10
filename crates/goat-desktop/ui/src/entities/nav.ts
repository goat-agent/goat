import type { IconName } from '../shared/icons';
import type { Mode, Route } from './routes';

export interface NavItem {
  id: string;
  label: string;
  icon: IconName;
  target: { kind: 'action'; action: 'new-chat' } | { kind: 'route'; route: Route };
}

export const NAV_ACTIONS: readonly NavItem[] = [
  { id: 'new-chat', label: 'New chat', icon: 'compose', target: { kind: 'action', action: 'new-chat' } },
];

export const NAV_LINKS: Record<Mode, readonly NavItem[]> = {
  code: [
    { id: 'integrations', label: 'Integrations', icon: 'plug', target: { kind: 'route', route: { kind: 'integrations' } } },
  ],
  agent: [
    { id: 'integrations', label: 'Integrations', icon: 'plug', target: { kind: 'route', route: { kind: 'integrations' } } },
    { id: 'memory', label: 'Memory', icon: 'brain', target: { kind: 'route', route: { kind: 'memory' } } },
  ],
};

export const MODES: readonly { id: Mode; label: string }[] = [
  { id: 'code', label: 'Code' },
  { id: 'agent', label: 'Agent' },
];

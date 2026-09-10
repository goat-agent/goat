export type Mode = 'code' | 'agent';

export type Route =
  | { kind: 'welcome' }
  | { kind: 'chat'; cwd: string; conversation: number | null; nonce?: number }
  | { kind: 'talk'; agent: string; conversation: string | null; nonce?: number }
  | { kind: 'integrations' }
  | { kind: 'memory' }
  | { kind: 'settings' };

export type ScreenKind = Extract<Route, { kind: 'integrations' | 'memory' | 'settings' }>['kind'];

export function routeKey(route: Route): string {
  switch (route.kind) {
    case 'chat':
      return `chat:${route.cwd}:${route.conversation ?? `new-${route.nonce ?? 0}`}`;
    case 'talk':
      return `talk:${route.agent}:${route.conversation ?? `new-${route.nonce ?? 0}`}`;
    default:
      return route.kind;
  }
}

export function modeOf(route: Route): Mode | null {
  return route.kind === 'chat' ? 'code' : route.kind === 'talk' ? 'agent' : null;
}

export function ownerOf(route: Route): string | null {
  return route.kind === 'chat' ? route.cwd : route.kind === 'talk' ? route.agent : null;
}

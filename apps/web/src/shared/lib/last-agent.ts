const KEY = "last-agent";

export function lastAgentId(): string | undefined {
  return localStorage.getItem(KEY) ?? undefined;
}

export function rememberAgent(agentId: string): void {
  localStorage.setItem(KEY, agentId);
}

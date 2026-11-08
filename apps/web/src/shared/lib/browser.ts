const INSTALLATION_KEY = "installation-id";

export function browserDevice(): { installationId: string; name: string; platform: "web" } {
  let installationId = localStorage.getItem(INSTALLATION_KEY);
  if (!installationId) {
    installationId = crypto.randomUUID();
    localStorage.setItem(INSTALLATION_KEY, installationId);
  }
  return { installationId, name: `${browserName()} · ${systemName()}`, platform: "web" };
}

function browserName(): string {
  const agent = navigator.userAgent;
  if (agent.includes("Edg/")) return "Edge";
  if (agent.includes("Firefox/")) return "Firefox";
  if (agent.includes("Chrome/")) return "Chrome";
  if (agent.includes("Safari/")) return "Safari";
  return "Browser";
}

function systemName(): string {
  const agent = navigator.userAgent;
  if (/iPhone|iPad/.test(agent)) return /iPad/.test(agent) ? "iPad" : "iPhone";
  if (agent.includes("Mac")) return "Mac";
  if (agent.includes("Android")) return "Android";
  if (agent.includes("Windows")) return "Windows";
  if (agent.includes("Linux")) return "Linux";
  return "Web";
}

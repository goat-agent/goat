import { CheckIcon } from "lucide-react";
import { useState } from "react";
import { IntegrationIcon, isConnected, useIntegrations } from "@/entities/integration";
import { api, type ConnectRequest, unwrap, expectOk } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { Button } from "@/shared/ui/button";

const RETURN_KEY = "connect-return";

/**
 * Leaves for the service's sign-in in this tab. The server sends the browser back to /connected,
 * which finishes and returns to the page saved here.
 */
export async function startConnecting(integrationId: string, agentId?: string): Promise<void> {
  const { url } = await unwrap(
    api.POST("/integrations/{id}/connect", {
      params: { path: { id: integrationId } },
      body: agentId ? { agentId } : {},
    }),
  );
  sessionStorage.setItem(RETURN_KEY, `${location.pathname}${location.search}`);
  location.assign(url);
}

/** Finishes a connection when the service sends the browser back. */
export async function finishConnecting(
  integrationId: string,
  state: string,
  code: string,
): Promise<void> {
  await expectOk(
    api.POST("/integrations/{id}/connections", {
      params: { path: { id: integrationId } },
      body: { state, code },
    }),
  );
}

/** The page saved before leaving, read once. */
export function takeReturnPath(): string {
  const path = sessionStorage.getItem(RETURN_KEY) ?? "/";
  sessionStorage.removeItem(RETURN_KEY);
  // Only a path on this site, never a full URL someone else wrote.
  return path.startsWith("/") && !path.startsWith("//") ? path : "/";
}

/** The card under a reply that asked the user to connect a service. */
export function ConnectCard({ request, agentId }: { request: ConnectRequest; agentId: string }) {
  const integration = useIntegrations().find(({ id }) => id === request.integrationId);
  const [isLeaving, setIsLeaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const connected = isConnected(integration);

  async function connect() {
    setIsLeaving(true);
    setFailed(false);
    try {
      await startConnecting(request.integrationId, agentId);
    } catch {
      setFailed(true);
      setIsLeaving(false);
    }
  }

  return (
    <div className="flex items-center gap-2.5 self-start rounded-xl border bg-background py-2 ps-3 pe-2">
      {integration && <IntegrationIcon src={integration.iconUrl} />}
      <span className="me-2 text-sm font-medium">{request.name}</span>
      {connected ? (
        <span className="flex items-center gap-1 pe-1 text-xs text-muted-foreground">
          <CheckIcon className="size-3.5" />
          {m.connected()}
        </span>
      ) : (
        <Button size="sm" onClick={() => void connect()} disabled={isLeaving}>
          {failed ? m.try_again() : m.connect()}
        </Button>
      )}
    </div>
  );
}

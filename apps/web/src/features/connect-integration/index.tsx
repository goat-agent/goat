import { useQueryClient } from "@tanstack/react-query";
import { CheckIcon } from "lucide-react";
import { useState } from "react";
import {
  IntegrationIcon,
  integrationsQuery,
  isConnected,
  useIntegrations,
} from "@/entities/integration";
import { api, type ConnectRequest, unwrap, expectOk } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import * as native from "@/shared/native";
import { Button } from "@/shared/ui/button";

const RETURN_KEY = "connect-return";

/**
 * Connects a service. A browser leaves for the service's sign-in in this tab, and the server sends
 * it back to /connected, which finishes and returns to the page saved here. The native app shows
 * the sign-in in its own sheet and hands back where it ended, so the page stays and finishes here.
 */
export function useConnect(): (integrationId: string, agentId?: string) => Promise<void> {
  const queryClient = useQueryClient();
  return async (integrationId, agentId) => {
    const { url } = await unwrap(
      api.POST("/integrations/{id}/connect", {
        params: { path: { id: integrationId } },
        body: agentId ? { agentId } : {},
      }),
    );
    if (!native.host) {
      sessionStorage.setItem(RETURN_KEY, `${location.pathname}${location.search}`);
      location.assign(url);
      return;
    }
    const { callbackUrl } = await native.request("authenticate", { url });
    if (!callbackUrl) return;
    const back = new URL(callbackUrl).searchParams;
    const state = back.get("state");
    const code = back.get("code");
    if (!state || !code) throw new Error(`connecting ${integrationId} ended without a code`);
    await finishConnecting(integrationId, state, code);
    await queryClient.invalidateQueries({ queryKey: integrationsQuery.queryKey });
  };
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
  const connectIntegration = useConnect();
  const [isLeaving, setIsLeaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const connected = isConnected(integration);

  async function connect() {
    setIsLeaving(true);
    setFailed(false);
    try {
      await connectIntegration(request.integrationId, agentId);
    } catch {
      setFailed(true);
    }
    setIsLeaving(false);
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

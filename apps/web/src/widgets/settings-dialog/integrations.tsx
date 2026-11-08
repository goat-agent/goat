import { useQueryClient } from "@tanstack/react-query";
import { PlusIcon } from "lucide-react";
import { useState } from "react";
import { startConnecting } from "@/features/connect-integration";
import { IntegrationIcon, integrationsQuery, useIntegrations } from "@/entities/integration";
import { api, expectOk, type Integration } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { Button } from "@/shared/ui/button";
import { Heading, List, Row } from "./rows";

export function Integrations() {
  const integrations = useIntegrations();
  const queryClient = useQueryClient();
  const [isAdding, setIsAdding] = useState(false);
  const connected = integrations.filter(({ connections }) => connections.length > 0);

  async function disconnect(connectionId: string) {
    await expectOk(api.DELETE("/connections/{id}", { params: { path: { id: connectionId } } }));
    await queryClient.invalidateQueries({ queryKey: integrationsQuery.queryKey });
  }

  if (isAdding) {
    return (
      <div className="flex flex-col gap-2">
        <Heading
          title={m.add_integration()}
          action={
            <Button variant="ghost" size="sm" onClick={() => setIsAdding(false)}>
              {m.done()}
            </Button>
          }
        />
        <List>
          {integrations.map((integration) => (
            <Row
              key={integration.id}
              icon={<IntegrationIcon src={integration.iconUrl} />}
              title={integration.name}
            >
              <Button
                variant="outline"
                size="sm"
                onClick={() => void startConnecting(integration.id)}
              >
                {m.connect()}
              </Button>
            </Row>
          ))}
        </List>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <Heading
        title={m.connected_services()}
        action={
          <Button variant="outline" size="sm" onClick={() => setIsAdding(true)}>
            <PlusIcon />
            {m.add_integration()}
          </Button>
        }
      />
      {connected.length === 0 ? (
        <p className="py-6 text-center text-sm text-muted-foreground">{m.no_integrations()}</p>
      ) : (
        <List>
          {connected.map((integration) => (
            <Accounts key={integration.id} integration={integration} onDisconnect={disconnect} />
          ))}
        </List>
      )}
    </div>
  );
}

function Accounts({
  integration,
  onDisconnect,
}: {
  integration: Integration;
  onDisconnect: (connectionId: string) => Promise<void>;
}) {
  return (
    <>
      <Row icon={<IntegrationIcon src={integration.iconUrl} />} title={integration.name}>
        <Button
          variant="ghost"
          size="sm"
          className="text-muted-foreground"
          onClick={() => void startConnecting(integration.id)}
        >
          {m.add_account()}
        </Button>
      </Row>
      {integration.connections.map((connection) => (
        <Row
          key={connection.id}
          isIndented
          title={connection.accountName}
          detail={connection.needsReconnect ? m.needs_reconnect() : undefined}
          isWarning={connection.needsReconnect}
        >
          {connection.needsReconnect ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => void startConnecting(integration.id)}
            >
              {m.reconnect()}
            </Button>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground"
              onClick={() => void onDisconnect(connection.id)}
            >
              {m.disconnect()}
            </Button>
          )}
        </Row>
      ))}
    </>
  );
}

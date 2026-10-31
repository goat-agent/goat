import { queryOptions, useQuery } from "@tanstack/react-query";
import { api, unwrap, type Integration } from "@/shared/api/client";

export { IntegrationIcon } from "./integration-icon";

export const integrationsQuery = queryOptions({
  queryKey: ["integrations"],
  queryFn: async () => (await unwrap(api.GET("/integrations"))).integrations,
});

export function useIntegrations(): Integration[] {
  return useQuery(integrationsQuery).data ?? [];
}

/** Connected with at least one account that still works. */
export function isConnected(integration: Integration | undefined): boolean {
  return integration?.connections.some(({ needsReconnect }) => !needsReconnect) ?? false;
}

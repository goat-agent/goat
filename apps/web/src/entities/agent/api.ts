import { type QueryClient, queryOptions, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { type Agent, api, unwrap } from "@/shared/api/client";
import { listen } from "@/shared/api/events";
import { AgentsEvent } from "@/shared/api/schemas";

export const agentsQuery = queryOptions({
  queryKey: ["agents"],
  queryFn: async () => (await unwrap(api.GET("/agents"))).agents,
});

export function useAgents(): Agent[] | undefined {
  return useQuery(agentsQuery).data;
}

export function useAgentListEvents(queryClient: QueryClient, onRefused: () => void): void {
  useEffect(
    () =>
      listen("/agents/events", ["agents"], AgentsEvent, {
        onEvent: ({ agents }) => queryClient.setQueryData(agentsQuery.queryKey, agents),
        onRefused,
      }),
    [queryClient, onRefused],
  );
}

export function showAgent(queryClient: QueryClient, agent: Agent): void {
  queryClient.setQueryData(agentsQuery.queryKey, (agents = []) =>
    agents.some(({ id }) => id === agent.id)
      ? agents.map((item) => (item.id === agent.id ? agent : item))
      : [agent, ...agents],
  );
}

export function hideAgent(queryClient: QueryClient, agentId: string): void {
  queryClient.setQueryData(agentsQuery.queryKey, (agents) =>
    agents?.filter(({ id }) => id !== agentId),
  );
}

export function isWorking(agent: Pick<Agent, "status">): boolean {
  return agent.status === "running";
}

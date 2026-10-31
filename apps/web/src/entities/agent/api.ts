import { type QueryClient, queryOptions, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { type Agent, api, unwrap } from "@/shared/api/client";
import { listen } from "@/shared/api/events";
import { AgentsEvent } from "@/shared/api/schemas";

export const agentsQuery = queryOptions({
  queryKey: ["agents"],
  queryFn: async () => (await unwrap(api.GET("/agents"))).agents,
});

/** The agents, newest conversation first; undefined until the list has loaded. */
export function useAgents(): Agent[] | undefined {
  return useQuery(agentsQuery).data;
}

/**
 * Keeps the agent list current. The server sends the whole list, statuses included, after every
 * change, so this stream is the one place the list and each agent's status come from.
 */
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

/**
 * Shows an agent the server just returned without waiting for the stream, which follows with the
 * same agent. A new agent goes first, where the server lists it.
 */
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

import {
  infiniteQueryOptions,
  type QueryClient,
  useInfiniteQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import { api, unwrap } from "@/shared/api/client";
import { listen } from "@/shared/api/events";
import { NewMessage } from "@/shared/api/schemas";
import { allMessages, type Conversation, type MessagePage, withMessage } from "./model";

const PAGE = 50;

export function messagesQuery(agentId: string) {
  return infiniteQueryOptions<
    MessagePage,
    Error,
    Conversation,
    readonly ["messages", string],
    number | undefined
  >({
    queryKey: ["messages", agentId],
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/agents/{id}/messages", {
          params: {
            path: { id: agentId },
            query: pageParam === undefined ? { limit: PAGE } : { before: pageParam, limit: PAGE },
          },
        }),
      ),
    initialPageParam: undefined,
    getPreviousPageParam: (first) =>
      first.messages.length === PAGE ? first.messages[0]?.id : undefined,
    getNextPageParam: () => undefined,
    // New messages arrive on the event stream, never by refetching.
    staleTime: Infinity,
  });
}

/** The newest message id in the cached conversation, or 0 before it has loaded. */
export function newestMessageId(queryClient: QueryClient, agentId: string): number {
  const conversation = queryClient.getQueryData<Conversation>(messagesQuery(agentId).queryKey);
  return allMessages(conversation).at(-1)?.id ?? 0;
}

/**
 * One agent's conversation, kept current: the newest page loads first, then the event stream
 * adds what comes after it. The agent's status comes from the agent list stream, not this one.
 */
export function useConversation(agentId: string, onRefused: () => void) {
  const queryClient = useQueryClient();
  const query = useInfiniteQuery(messagesQuery(agentId));
  const isLoaded = query.isSuccess;

  useEffect(() => {
    if (!isLoaded) return undefined;
    const after = newestMessageId(queryClient, agentId);
    const key = messagesQuery(agentId).queryKey;
    const path = `/agents/${agentId}/events${after > 0 ? `?after=${after}` : ""}`;
    return listen(path, ["message"], NewMessage, {
      onEvent: ({ message }) =>
        queryClient.setQueryData<Conversation>(key, (current) => withMessage(current, message)),
      onRefused,
    });
  }, [agentId, isLoaded, queryClient, onRefused]);

  return { ...query, messages: allMessages(query.data) };
}

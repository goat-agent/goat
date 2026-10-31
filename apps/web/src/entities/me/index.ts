import { queryOptions, useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import { api, expectOk, unwrap, type User } from "@/shared/api/client";

export const meQuery = queryOptions({
  queryKey: ["me"],
  queryFn: () => unwrap(api.GET("/me")),
  staleTime: 60_000,
});

export function useMe(): User {
  return useSuspenseQuery(meQuery).data;
}

/**
 * Asks the server again whether the session is still good, for when a stream was refused. A 401
 * there sends the signed-in shell to the sign-in page.
 */
export function useRecheckSession(): () => void {
  const queryClient = useQueryClient();
  return useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: meQuery.queryKey });
  }, [queryClient]);
}

/** Fills in the user's time zone once, so scheduled work runs at their local time. */
export function useFillTimeZone(): void {
  const me = useQuery(meQuery).data;
  const queryClient = useQueryClient();
  useEffect(() => {
    if (!me || me.timeZone) return;
    const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    void expectOk(api.PATCH("/me", { body: { timeZone } })).then(() =>
      queryClient.invalidateQueries({ queryKey: meQuery.queryKey }),
    );
  }, [me, queryClient]);
}

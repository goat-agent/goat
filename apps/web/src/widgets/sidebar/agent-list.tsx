import { Link } from "@tanstack/react-router";
import { PhoneIcon } from "lucide-react";
import type { ReactNode } from "react";
import { type Call, useCall } from "@/features/call";
import { AgentAvatar, isWorking } from "@/entities/agent";
import type { Agent } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { listTime } from "@/shared/lib/time";
import { Dots } from "@/shared/ui/dots";

/**
 * What a row says about its agent, most pressing first. A new state (a new reply, waiting for
 * the user, a failed task) is a new case in `rowState` and a new entry in `rows`.
 */
type RowState = "inCall" | "working" | "idle";

function rowState(agent: Agent, call: Call | undefined): RowState {
  if (call?.agentId === agent.id && call.phase !== "failed") return "inCall";
  if (isWorking(agent)) return "working";
  return "idle";
}

// `line` replaces the last message under the name. In the column of faces that line is hidden,
// so `ring` puts a moving ring around the face instead.
const rows: Record<RowState, { line: (agent: Agent) => ReactNode; ring: boolean }> = {
  inCall: {
    line: () => (
      <span className="flex items-center gap-1 text-foreground">
        <PhoneIcon className="size-3.5" />
        {m.in_call()}
      </span>
    ),
    ring: true,
  },
  working: {
    line: () => (
      <span aria-label={m.working()}>
        <Dots />
      </span>
    ),
    ring: true,
  },
  idle: {
    line: (agent) => <span className="truncate">{agent.lastMessage?.text}</span>,
    ring: false,
  },
};

export function AgentList({ agents, isPage }: { agents: Agent[]; isPage: boolean }) {
  const call = useCall();
  return (
    <nav aria-label={m.agents()} className="flex flex-col gap-0.5">
      {agents.map((agent) => {
        const row = rows[rowState(agent, call)];
        return (
          <Link
            key={agent.id}
            to="/agents/$agentId"
            params={{ agentId: agent.id }}
            search={{}}
            title={agent.name}
            className={`flex items-center gap-2.5 rounded-lg p-2 hover:bg-sidebar-accent group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:p-1 ${isPage ? "min-h-17 py-2.5" : "min-h-14 data-[status=active]:bg-sidebar-accent"}`}
          >
            <span className="relative shrink-0">
              <AgentAvatar
                agent={agent}
                className={`${isPage ? "size-12 text-lg" : "size-10"} group-data-[collapsible=icon]:size-11`}
              />
              {row.ring && (
                <span
                  aria-hidden
                  className="absolute -inset-1 hidden animate-spin rounded-full border-2 border-transparent border-t-foreground border-r-foreground group-data-[collapsible=icon]:block motion-reduce:animate-none"
                />
              )}
            </span>
            <span className="min-w-0 flex-1 group-data-[collapsible=icon]:sr-only">
              <span className="flex items-center gap-1.5">
                <span className={`truncate font-medium ${isPage ? "text-[15px]" : ""}`}>
                  {agent.name}
                </span>
                {agent.lastMessage && (
                  <span className="ms-auto shrink-0 text-xs text-muted-foreground">
                    {listTime(agent.lastMessage.createdAt)}
                  </span>
                )}
              </span>
              <span
                className={`mt-0.5 flex h-[18px] items-center gap-1 truncate text-muted-foreground ${isPage ? "text-sm" : "text-[13px]"}`}
              >
                {row.line(agent)}
              </span>
            </span>
          </Link>
        );
      })}
    </nav>
  );
}

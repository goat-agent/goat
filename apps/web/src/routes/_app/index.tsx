import { createFileRoute, Link, Navigate } from "@tanstack/react-router";
import { AgentListPage } from "@/widgets/sidebar";
import { useAgents } from "@/entities/agent";
import { m } from "@/shared/i18n/paraglide/messages";
import { lastAgentId } from "@/shared/lib/last-agent";
import { useWindowClass } from "@/shared/lib/window-class";
import { Button } from "@/shared/ui/button";

export const Route = createFileRoute("/_app/")({ component: Home });

/** A narrow screen starts at the list; a wide one goes back to the last conversation. */
function Home() {
  const windowClass = useWindowClass();
  const agents = useAgents();
  if (!agents) return null;
  if (windowClass === "compact" && agents.length > 0) return <AgentListPage />;

  const target = agents.find(({ id }) => id === lastAgentId()) ?? agents[0];
  if (target) {
    return (
      <Navigate
        to="/agents/$agentId"
        params={{ agentId: target.id }}
        search={(previous) => previous}
        replace
      />
    );
  }
  return (
    <main className="grid flex-1 place-items-center p-6 text-center">
      <div className="flex flex-col items-center gap-3">
        <p className="text-xl font-semibold">{m.no_agents()}</p>
        <p className="text-sm text-muted-foreground">{m.no_agents_hint()}</p>
        <Button asChild className="mt-2">
          <Link to="." search={(previous) => ({ ...previous, dialog: "new" as const })}>
            {m.new_agent()}
          </Link>
        </Button>
      </div>
    </main>
  );
}

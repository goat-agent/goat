import { createFileRoute } from "@tanstack/react-router";
import { useEffect } from "react";
import { ConversationPane } from "@/widgets/conversation";
import { rememberAgent } from "@/shared/lib/last-agent";

export const Route = createFileRoute("/_app/agents/$agentId")({ component: Agent });

function Agent() {
  const { agentId } = Route.useParams();
  useEffect(() => rememberAgent(agentId), [agentId]);
  return <ConversationPane agentId={agentId} />;
}

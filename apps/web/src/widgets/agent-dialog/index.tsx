import { AgentSettings, NewAgentForm } from "@/features/edit-agent";
import { useAgents } from "@/entities/agent";
import { m } from "@/shared/i18n/paraglide/messages";
import { ResponsiveDialog } from "@/shared/ui/responsive-dialog";

export function AgentDialog({ agentId, onClose }: { agentId: string; onClose: () => void }) {
  const agent = useAgents()?.find(({ id }) => id === agentId);
  if (!agent) return null;
  return (
    <ResponsiveDialog title={agent.name} size="form" onClose={onClose}>
      <div className="overflow-y-auto p-5">
        <AgentSettings agent={agent} />
      </div>
    </ResponsiveDialog>
  );
}

export function NewAgentDialog({ onClose }: { onClose: () => void }) {
  return (
    <ResponsiveDialog title={m.new_agent()} size="form" onClose={onClose}>
      <div className="overflow-y-auto p-5">
        <NewAgentForm onCancel={onClose} />
      </div>
    </ResponsiveDialog>
  );
}

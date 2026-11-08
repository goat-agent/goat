import { cn } from "cn";
import { type Agent, avatarUrl } from "@/shared/api/client";
import { Avatar, AvatarFallback, AvatarImage } from "@/shared/ui/avatar";

export function AgentAvatar({
  agent,
  className,
}: {
  agent: Pick<Agent, "id" | "name" | "avatarId">;
  className?: string;
}) {
  return (
    <Avatar className={cn("size-10", className)}>
      <AvatarImage src={avatarUrl(agent)} alt="" />
      <AvatarFallback className="bg-muted font-semibold text-muted-foreground">
        {Array.from(agent.name)[0] ?? ""}
      </AvatarFallback>
    </Avatar>
  );
}

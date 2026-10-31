import { cn } from "cn";
import { type Agent, avatarUrl } from "@/shared/api/client";
import { Avatar, AvatarFallback, AvatarImage } from "@/shared/ui/avatar";

/** An agent's face: its picture, or the first letter of its name until it has one. */
export function AgentAvatar({
  agent,
  className,
}: {
  agent: Pick<Agent, "id" | "name" | "avatarId">;
  className?: string;
}) {
  const url = avatarUrl(agent);
  return (
    <Avatar className={cn("size-10", className)}>
      {url && <AvatarImage src={url} alt="" />}
      <AvatarFallback className="bg-muted font-semibold text-muted-foreground">
        {Array.from(agent.name)[0] ?? ""}
      </AvatarFallback>
    </Avatar>
  );
}

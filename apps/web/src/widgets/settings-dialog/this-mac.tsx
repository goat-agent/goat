import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckIcon } from "lucide-react";
import { m } from "@/shared/i18n/paraglide/messages";
import * as native from "@/shared/native";
import { Button } from "@/shared/ui/button";
import { Switch } from "@/shared/ui/switch";
import { Heading, List, Row } from "./rows";

const computerUseQuery = queryOptions({
  queryKey: ["computer-use"],
  queryFn: () => native.request("computerUse", {}),
  // macOS gives no notice when a permission changes, so the page asks again while it shows them.
  refetchInterval: 1000,
});

/** Whether the agent may use this Mac, and the two permissions that takes. */
export function ThisMac() {
  const queryClient = useQueryClient();
  const status = useQuery(computerUseQuery).data;
  if (!status) return null;

  async function setEnabled(isEnabled: boolean) {
    const updated = await native.request("setComputerUse", { isEnabled });
    queryClient.setQueryData(computerUseQuery.queryKey, updated);
  }

  return (
    <div className="flex flex-col gap-2">
      <List>
        <Row title={m.let_agent_use_mac()} detail={m.let_agent_use_mac_hint()}>
          <Switch
            checked={status.isEnabled}
            onCheckedChange={(isOn: boolean) => void setEnabled(isOn)}
            aria-label={m.let_agent_use_mac()}
          />
        </Row>
      </List>
      <Heading title={m.permissions()} />
      <List>
        <Permission
          title={m.accessibility()}
          isGranted={status.permissions.accessibility}
          onOpen={() =>
            void native.request("openPermissionSettings", { permission: "accessibility" })
          }
        />
        <Permission
          title={m.screen_recording()}
          isGranted={status.permissions.screenRecording}
          onOpen={() =>
            void native.request("openPermissionSettings", { permission: "screenRecording" })
          }
        />
      </List>
      <p className="text-xs text-muted-foreground">{m.permissions_hint()}</p>
    </div>
  );
}

function Permission({
  title,
  isGranted,
  onOpen,
}: {
  title: string;
  isGranted: boolean;
  onOpen: () => void;
}) {
  return (
    <Row title={title}>
      {isGranted ? (
        <span className="flex items-center gap-1 text-sm text-muted-foreground">
          <CheckIcon className="size-4" />
          {m.permission_on()}
        </span>
      ) : (
        <Button variant="outline" size="sm" onClick={onOpen}>
          {m.open_system_settings()}
        </Button>
      )}
    </Row>
  );
}

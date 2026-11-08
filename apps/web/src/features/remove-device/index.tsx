import { useQueryClient } from "@tanstack/react-query";
import { devicesQuery } from "@/entities/device";
import { api, type Device, expectOk } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";

export function RemoveDeviceButton({ device }: { device: Device }) {
  const queryClient = useQueryClient();

  async function remove() {
    await expectOk(api.DELETE("/devices/{id}", { params: { path: { id: device.id } } }));
    await queryClient.invalidateQueries({ queryKey: devicesQuery.queryKey });
  }

  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="ghost" size="sm" className="text-muted-foreground">
          {m.remove()}
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{m.remove_device_title({ name: device.name })}</AlertDialogTitle>
          <AlertDialogDescription>{m.remove_device_body()}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{m.cancel()}</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={() => void remove()}>
            {m.remove()}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

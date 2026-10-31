import { RemoveDeviceButton } from "@/features/remove-device";
import { DeviceIcon, useDevices } from "@/entities/device";
import { m } from "@/shared/i18n/paraglide/messages";
import { Heading, List, Row } from "./rows";

export function Devices() {
  const devices = useDevices();
  return (
    <div className="flex flex-col gap-2">
      <Heading title={m.signed_in_devices()} />
      <List>
        {devices.map((device) => (
          <Row
            key={device.id}
            icon={<DeviceIcon platform={device.platform} />}
            title={device.name}
            detail={device.isCurrent ? m.this_device() : undefined}
          >
            {!device.isCurrent && <RemoveDeviceButton device={device} />}
          </Row>
        ))}
      </List>
      <p className="text-xs text-muted-foreground">{m.remove_device_hint()}</p>
    </div>
  );
}

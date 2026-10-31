import { GlobeIcon, LaptopIcon, MonitorIcon, SmartphoneIcon } from "lucide-react";

export function DeviceIcon({ platform }: { platform: string }) {
  const Icon =
    platform === "ios"
      ? SmartphoneIcon
      : platform === "macos"
        ? LaptopIcon
        : platform === "web"
          ? GlobeIcon
          : MonitorIcon;
  return (
    <span className="grid size-8 shrink-0 place-items-center rounded-[9px] bg-muted text-muted-foreground">
      <Icon className="size-4" />
    </span>
  );
}

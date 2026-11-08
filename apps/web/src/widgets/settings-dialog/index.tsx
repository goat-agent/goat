import {
  ChevronLeftIcon,
  ChevronRightIcon,
  LaptopIcon,
  PlugIcon,
  SunIcon,
  UserIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import { AccountForm } from "@/features/account";
import { endCall } from "@/features/call";
import { m } from "@/shared/i18n/paraglide/messages";
import { useWindowClass } from "@/shared/lib/window-class";
import { ResponsiveDialog } from "@/shared/ui/responsive-dialog";
import { Devices } from "./devices";
import { General } from "./general";
import { Integrations } from "./integrations";

export const settingsTabs = ["general", "account", "integrations", "devices"] as const;
export type SettingsTab = (typeof settingsTabs)[number];

const sections: Record<
  SettingsTab,
  { label: () => string; icon: ReactNode; body: () => ReactNode }
> = {
  general: { label: m.general, icon: <SunIcon />, body: () => <General /> },
  account: {
    label: m.account,
    icon: <UserIcon />,
    body: () => <AccountForm beforeSignOut={endCall} />,
  },
  integrations: { label: m.integrations, icon: <PlugIcon />, body: () => <Integrations /> },
  devices: { label: m.devices, icon: <LaptopIcon />, body: () => <Devices /> },
};

export function SettingsDialog({
  tab,
  onTab,
  onClose,
}: {
  tab: SettingsTab | undefined;
  onTab: (tab: SettingsTab | undefined) => void;
  onClose: () => void;
}) {
  const isCompact = useWindowClass() === "compact";
  const shown = tab ?? "general";
  const isInSection = isCompact && tab !== undefined;

  return (
    <ResponsiveDialog
      title={isInSection ? sections[shown].label() : m.settings()}
      leading={
        isInSection && (
          <button
            type="button"
            onClick={() => onTab(undefined)}
            aria-label={m.settings()}
            className="-ms-3 grid size-11 place-items-center text-muted-foreground"
          >
            <ChevronLeftIcon className="size-5" />
          </button>
        )
      }
      size="panel"
      onClose={onClose}
    >
      <div className="flex min-h-0 flex-1">
        {!isInSection && (
          <nav
            aria-label={m.settings()}
            className="flex w-full flex-col gap-0.5 p-3 md:w-46 md:shrink-0 md:border-e md:px-2"
          >
            {settingsTabs.map((id) => (
              <button
                key={id}
                type="button"
                onClick={() => onTab(id)}
                aria-current={!isCompact && id === shown ? "page" : undefined}
                className="flex min-h-11 items-center gap-2.5 rounded-lg px-2.5 font-medium text-muted-foreground hover:bg-muted hover:text-foreground aria-[current=page]:bg-muted aria-[current=page]:text-foreground md:min-h-9 [&_svg]:size-4"
              >
                {sections[id].icon}
                {sections[id].label()}
                {isCompact && <ChevronRightIcon className="ms-auto" />}
              </button>
            ))}
          </nav>
        )}
        {(isInSection || !isCompact) && (
          <div className="min-w-0 flex-1 overflow-y-auto p-5">{sections[shown].body()}</div>
        )}
      </div>
    </ResponsiveDialog>
  );
}

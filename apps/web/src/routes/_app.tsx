import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Navigate, Outlet, useParams } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { z } from "zod";
import { AgentDialog, NewAgentDialog } from "@/widgets/agent-dialog";
import { SettingsDialog, settingsTabs } from "@/widgets/settings-dialog";
import { AppSidebar } from "@/widgets/sidebar";
import { useAgentListEvents } from "@/entities/agent";
import { meQuery, useFillTimeZone, useRecheckSession } from "@/entities/me";
import { ApiError } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { useWindowClass } from "@/shared/lib/window-class";
import { host, onNativeEvent } from "@/shared/native";
import { SidebarInset, SidebarProvider } from "@/shared/ui/sidebar";

// Dialogs live in the search so a reload or a link reopens them over the same page.
const Search = z.object({
  dialog: z.enum(["settings", "agent", "new"]).optional().catch(undefined),
  tab: z.enum(settingsTabs).optional().catch(undefined),
});

export const Route = createFileRoute("/_app")({
  validateSearch: Search,
  component: SignedIn,
});

function SignedIn() {
  const me = useQuery(meQuery);
  if (me.isPending) return null;
  if (me.isError) {
    if (me.error instanceof ApiError && me.error.status === 401) {
      return <Navigate to="/sign-in" search={{}} replace />;
    }
    return (
      <main className="grid min-h-dvh place-items-center p-6 text-sm text-muted-foreground">
        {m.unreachable()}
      </main>
    );
  }
  return <Shell />;
}

const SIDEBAR_KEY = "sidebar-open";

function Shell() {
  const queryClient = useQueryClient();
  const navigate = Route.useNavigate();
  useEffect(
    () =>
      onNativeEvent("openSettings", () => {
        void navigate({ search: (previous) => ({ ...previous, dialog: "settings" as const }) });
      }),
    [navigate],
  );
  useAgentListEvents(queryClient, useRecheckSession());
  useFillTimeZone();
  const windowClass = useWindowClass();
  const [isOpen, setIsOpen] = useState(() => localStorage.getItem(SIDEBAR_KEY) !== "false");

  const dialogs = <Dialogs />;
  if (windowClass === "compact") {
    return (
      <div className="flex h-dvh flex-col bg-background">
        <Outlet />
        {dialogs}
      </div>
    );
  }
  return (
    <SidebarProvider
      // A medium window always shows the column of faces; only an expanded one can open fully.
      open={windowClass === "expanded" && isOpen}
      onOpenChange={(open) => {
        if (windowClass !== "expanded") return;
        setIsOpen(open);
        localStorage.setItem(SIDEBAR_KEY, String(open));
      }}
      style={{
        "--sidebar-width": "17rem",
        // Wide enough for the Mac window's buttons, which sit at the top of the column.
        "--sidebar-width-icon": host?.platform === "macos" ? "4.875rem" : "3.25rem",
      }}
      className="h-dvh min-h-0"
    >
      <AppSidebar />
      <SidebarInset className="min-h-0 overflow-hidden">
        <Outlet />
      </SidebarInset>
      {dialogs}
    </SidebarProvider>
  );
}

function Dialogs() {
  const { dialog, tab } = Route.useSearch();
  const navigate = Route.useNavigate();
  const { agentId } = useParams({ strict: false });
  const close = () =>
    void navigate({ search: (previous) => ({ ...previous, dialog: undefined, tab: undefined }) });

  if (dialog === "settings") {
    return (
      <SettingsDialog
        tab={tab}
        onTab={(next) => void navigate({ search: (previous) => ({ ...previous, tab: next }) })}
        onClose={close}
      />
    );
  }
  if (dialog === "new") return <NewAgentDialog onClose={close} />;
  if (dialog === "agent" && agentId) return <AgentDialog agentId={agentId} onClose={close} />;
  return null;
}

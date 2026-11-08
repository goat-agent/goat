import { Link } from "@tanstack/react-router";
import { SearchIcon, SquarePenIcon, XIcon } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useAgents } from "@/entities/agent";
import { useMe } from "@/entities/me";
import { m } from "@/shared/i18n/paraglide/messages";
import { Avatar, AvatarFallback } from "@/shared/ui/avatar";
import { Sidebar, SidebarContent, SidebarFooter, SidebarHeader } from "@/shared/ui/sidebar";
import { AgentList } from "./agent-list";

const ICON_BUTTON =
  "grid size-9 place-items-center rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground pointer-coarse:size-11";

function AgentPanel({
  isPage,
  frame,
}: {
  isPage: boolean;
  frame: (parts: { header: ReactNode; list: ReactNode; footer: ReactNode }) => ReactNode;
}) {
  const [query, setQuery] = useState<string>();
  const needle = query?.trim().toLocaleLowerCase() ?? "";
  const agents = (useAgents() ?? []).filter(
    ({ name }) => !needle || name.toLocaleLowerCase().includes(needle),
  );

  const header =
    query === undefined ? (
      <>
        <button
          type="button"
          onClick={() => setQuery("")}
          aria-label={m.search()}
          className={ICON_BUTTON}
        >
          <SearchIcon className="size-4" />
        </button>
        <Link
          to="."
          search={(previous) => ({ ...previous, dialog: "new" as const })}
          aria-label={m.new_agent()}
          className={ICON_BUTTON}
        >
          <SquarePenIcon className="size-4" />
        </Link>
      </>
    ) : (
      <div className="flex h-9 flex-1 items-center gap-2 rounded-md border bg-background px-2.5 text-muted-foreground">
        <SearchIcon className="size-4 shrink-0" />
        <input
          autoFocus
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") setQuery(undefined);
          }}
          placeholder={m.search()}
          aria-label={m.search()}
          className="min-w-0 flex-1 bg-transparent text-sm text-foreground outline-none"
        />
        <button type="button" onClick={() => setQuery(undefined)} aria-label={m.close()}>
          <XIcon className="size-4" />
        </button>
      </div>
    );

  return frame({
    header,
    list: <AgentList agents={agents} isPage={isPage} />,
    footer: <MeButton isPage={isPage} />,
  });
}

function MeButton({ isPage }: { isPage: boolean }) {
  const me = useMe();
  const label = me.name || me.email;
  return (
    <Link
      to="."
      search={(previous) => ({ ...previous, dialog: "settings" as const, tab: "account" as const })}
      title={m.settings()}
      className={`flex items-center gap-2.5 rounded-lg p-2 hover:bg-sidebar-accent group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:p-1.5 ${isPage ? "py-2.5" : ""}`}
    >
      <Avatar className="size-7 group-data-[collapsible=icon]:size-8">
        <AvatarFallback className="bg-foreground text-xs font-semibold text-background">
          {Array.from(label)[0]?.toLocaleUpperCase() ?? ""}
        </AvatarFallback>
      </Avatar>
      <span className="truncate text-[13px] font-medium group-data-[collapsible=icon]:sr-only">
        {label}
      </span>
    </Link>
  );
}

export function AppSidebar() {
  return (
    <AgentPanel
      isPage={false}
      frame={({ header, list, footer }) => (
        <Sidebar variant="inset" collapsible="icon">
          <SidebarHeader className="flex-row items-center justify-end gap-0.5 px-1 group-data-[collapsible=icon]:flex-col">
            {header}
          </SidebarHeader>
          <SidebarContent className="px-1">{list}</SidebarContent>
          <SidebarFooter className="px-1">{footer}</SidebarFooter>
        </Sidebar>
      )}
    />
  );
}

export function AgentListPage() {
  return (
    <AgentPanel
      isPage
      frame={({ header, list, footer }) => (
        <div className="flex min-h-0 flex-1 flex-col px-2">
          <header className="flex h-13 shrink-0 items-center justify-end gap-0.5 ps-2">
            {header}
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto">{list}</div>
          <footer className="pb-[max(0.5rem,env(safe-area-inset-bottom))]">{footer}</footer>
        </div>
      )}
    />
  );
}

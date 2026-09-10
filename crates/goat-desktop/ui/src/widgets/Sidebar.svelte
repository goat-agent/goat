<script lang="ts">
  import type { AgentConversationEntry, AgentEntry, ConversationInfo } from '../shared/api';
  import type { Project } from '../shared/ipc';
  import { MODES, NAV_ACTIONS, NAV_LINKS, type NavItem } from '../entities/nav';
  import { routeKey, type Mode, type Route } from '../entities/routes';
  import { popupMenu, type MenuEntry } from '../features/menu';
  import Icon from '../shared/Icon.svelte';
  import IconButton from '../shared/IconButton.svelte';
  import Row from '../shared/Row.svelte';
  import WindowChrome from './WindowChrome.svelte';

  let {
    mode,
    route,
    collapsed,
    canBack,
    canForward,
    projects,
    conversations,
    agents,
    agentConversations,
    expanded,
    busyConversation,
    onnavigate,
    onmode,
    onnewchat,
    ontoggleexpanded,
    onaddproject,
    onremoveproject,
    onsearch,
    ontoggle,
    onback,
    onforward,
    onerror,
  }: {
    mode: Mode;
    route: Route;
    collapsed: boolean;
    canBack: boolean;
    canForward: boolean;
    projects: Project[];
    conversations: Record<string, ConversationInfo[] | undefined>;
    agents: AgentEntry[];
    agentConversations: Record<string, AgentConversationEntry[] | undefined>;
    expanded: string[];
    busyConversation: number | null;
    onnavigate: (route: Route) => void;
    onmode: (mode: Mode) => void;
    onnewchat: (owner?: string) => void;
    ontoggleexpanded: (key: string) => void;
    onaddproject: () => void;
    onremoveproject: (path: string) => void;
    onsearch: () => void;
    ontoggle: () => void;
    onback: () => void;
    onforward: () => void;
    onerror: (message: string) => void;
  } = $props();

  const current = $derived(routeKey(route));
  const links = $derived(NAV_LINKS[mode]);
  const basename = (path: string) => path.split('/').filter(Boolean).at(-1) ?? path;

  function report(promise: Promise<unknown>) {
    void promise.catch((error: unknown) => onerror(error instanceof Error ? error.message : String(error)));
  }

  function navigate(item: NavItem) {
    if (item.target.kind === 'action') onnewchat();
    else onnavigate(item.target.route);
  }

  function modeMenu(anchor: HTMLElement) {
    report(popupMenu(MODES.map(entry => ({
      text: entry.id === mode ? `✓  ${entry.label}` : `    ${entry.label}`,
      action: () => onmode(entry.id),
    })), anchor));
  }

  function projectMenu(project: Project, anchor?: HTMLElement | null) {
    const entries: MenuEntry[] = [
      { text: 'New chat', action: () => onnewchat(project.path) },
      { text: 'Rename…', enabled: false },
      { separator: true },
      { text: 'Remove from projects', action: () => onremoveproject(project.path) },
    ];
    report(popupMenu(entries, anchor));
  }

  function conversationMenu(cwd: string, conversation: ConversationInfo, anchor?: HTMLElement | null) {
    const entries: MenuEntry[] = [
      { text: 'Open', action: () => onnavigate({ kind: 'chat', cwd, conversation: conversation.conversation_id }) },
      { text: 'Rename…', enabled: false },
      { separator: true },
      { text: 'Pin chat', enabled: false },
      { text: 'Archive chat', enabled: false },
      { separator: true },
      { text: 'Delete chat', enabled: false },
    ];
    report(popupMenu(entries, anchor));
  }

  function threadMenu(slug: string, thread: AgentConversationEntry, anchor?: HTMLElement | null) {
    const entries: MenuEntry[] = [
      { text: 'Open', action: () => onnavigate({ kind: 'talk', agent: slug, conversation: thread.conversation }) },
      { text: 'Rename…', enabled: false },
      { separator: true },
      { text: 'Pin chat', enabled: false },
      { text: 'Archive chat', enabled: false },
      { separator: true },
      { text: 'Delete chat', enabled: false },
    ];
    report(popupMenu(entries, anchor));
  }

  function agentMenu(agent: AgentEntry, anchor?: HTMLElement | null) {
    const entries: MenuEntry[] = [
      { text: 'New chat', action: () => onnewchat(agent.slug) },
      { separator: true },
      { text: 'Open agent.md', enabled: false },
      { text: 'Reload agent', enabled: false },
    ];
    report(popupMenu(entries, anchor));
  }

  function sectionMenu() {
    if (mode !== 'code') return;
    report(popupMenu([{ text: 'Add project folder…', action: onaddproject }]));
  }
</script>

<aside
  class={[
    'group/sb col-start-1 row-start-1 row-span-2 flex min-h-0 flex-col overflow-hidden',
    'border-r border-hairline bg-surface',
  ]}
  inert={collapsed}
  aria-hidden={collapsed}
>
  <WindowChrome {collapsed} {canBack} {canForward} {ontoggle} {onback} {onforward} />

  <div class="flex items-center pt-1 pr-2.5 pb-2.5 pl-[11px]">
    <button
      type="button"
      class="flex h-7 items-center gap-1 rounded-md pr-1.5 pl-[7px] text-lg font-[570] tracking-[-.25px] text-fg-1 transition-colors hover:bg-fill-hover active:bg-fill-active"
      aria-haspopup="menu"
      title="Switch mode · ⌘1 / ⌘2"
      onclick={(event) => modeMenu(event.currentTarget)}
    >
      {mode === 'code' ? 'Code' : 'Agent'}
      <Icon name="down" size={12} class="text-fg-3" />
    </button>
    <span class="flex-1"></span>
    <IconButton icon="search" label="Search" hint="Search · ⌘K" onclick={onsearch} />
  </div>

  <nav class="px-2">
    {#each NAV_ACTIONS as item (item.id)}
      <Row label={item.label} icon={item.icon} onclick={() => navigate(item)} />
    {/each}
  </nav>

  <nav class="px-2 pt-2.5">
    {#each links as item (item.id)}
      <Row
        label={item.label}
        icon={item.icon}
        active={item.target.kind === 'route' && routeKey(item.target.route) === current}
        onclick={() => navigate(item)}
      />
    {/each}
  </nav>

  <div class="mt-3.5 flex h-6 items-center pr-2.5 pl-[18px]">
    <h2 class="flex-1 text-xs font-[450] text-fg-3">{mode === 'code' ? 'Projects' : 'Agents'}</h2>
    {#if mode === 'code'}
      <IconButton
        icon="plus"
        label="Add project"
        hint="Add project folder"
        class="opacity-0 group-hover/sb:opacity-100 focus-visible:opacity-100"
        onclick={onaddproject}
      />
    {/if}
  </div>

  <div
    class="min-h-0 flex-1 overflow-y-auto px-2 pb-3"
    oncontextmenu={(event) => { if (event.target === event.currentTarget) { event.preventDefault(); sectionMenu(); } }}
    role="presentation"
  >
    {#if mode === 'code'}
      {#each projects as project (project.path)}
        {@const open = expanded.includes(project.path)}
        {@const rows = conversations[project.path]}
        <Row
          label={basename(project.path)}
          icon={open ? 'folderOpen' : 'folder'}
          disclosure
          {open}
          menu={`Actions for ${basename(project.path)}`}
          onclick={() => ontoggleexpanded(project.path)}
          onmenu={(anchor) => projectMenu(project, anchor)}
          oncontext={() => projectMenu(project)}
        />
        {#if open}
          {#if rows === undefined}
            <p class="flex h-[30px] items-center pl-[35px] text-fg-4">Loading…</p>
          {:else if rows.length === 0}
            <p class="flex h-[30px] items-center pl-[35px] text-fg-3">No chats</p>
          {:else}
            {#each rows as conversation (conversation.conversation_id)}
              <Row
                label={conversation.title || 'Untitled'}
                indent
                active={current === routeKey({ kind: 'chat', cwd: project.path, conversation: conversation.conversation_id })}
                busy={busyConversation === conversation.conversation_id}
                menu={`Actions for ${conversation.title || 'Untitled'}`}
                onclick={() => onnavigate({ kind: 'chat', cwd: project.path, conversation: conversation.conversation_id })}
                onmenu={(anchor) => conversationMenu(project.path, conversation, anchor)}
                oncontext={() => conversationMenu(project.path, conversation)}
              />
            {/each}
          {/if}
        {/if}
      {/each}
      {#if !projects.length}
        <p class="px-2.5 py-3 leading-relaxed text-fg-3">A home for every project. Add a folder to begin.</p>
      {/if}
    {:else}
      {#each agents as agent (agent.slug)}
        {@const open = expanded.includes(agent.slug)}
        {@const threads = agentConversations[agent.slug]}
        <Row
          label={agent.display || agent.slug}
          icon="bot"
          disclosure
          {open}
          menu={`Actions for ${agent.display || agent.slug}`}
          onclick={() => ontoggleexpanded(agent.slug)}
          onmenu={(anchor) => agentMenu(agent, anchor)}
          oncontext={() => agentMenu(agent)}
        />
        {#if open}
          {#if threads === undefined}
            <p class="flex h-[30px] items-center pl-[35px] text-fg-4">Loading…</p>
          {:else if threads.length === 0}
            <p class="flex h-[30px] items-center pl-[35px] text-fg-3">No chats</p>
          {:else}
            {#each threads as thread (thread.conversation)}
              <Row
                label={thread.title || 'Untitled'}
                indent
                active={current === routeKey({ kind: 'talk', agent: agent.slug, conversation: thread.conversation })}
                menu={`Actions for ${thread.title || 'Untitled'}`}
                onclick={() => onnavigate({ kind: 'talk', agent: agent.slug, conversation: thread.conversation })}
                onmenu={(anchor) => threadMenu(agent.slug, thread, anchor)}
                oncontext={() => threadMenu(agent.slug, thread)}
              />
            {/each}
          {/if}
        {/if}
      {/each}
      {#if !agents.length}
        <p class="px-2.5 py-3 leading-relaxed text-fg-3">No agents configured. Create one with the goat CLI.</p>
      {/if}
    {/if}
  </div>

  <footer class="shrink-0 px-2 pt-1 pb-2">
    <Row
      label="Settings"
      icon="settings"
      active={route.kind === 'settings'}
      onclick={() => onnavigate({ kind: 'settings' })}
    />
  </footer>
</aside>

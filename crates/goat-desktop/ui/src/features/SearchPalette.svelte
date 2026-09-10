<script lang="ts">
  import type { AgentEntry, ConversationInfo } from '../shared/api';
  import type { Project } from '../shared/ipc';
  import type { IconName } from '../shared/icons';
  import type { Mode, Route } from '../entities/routes';
  import Icon from '../shared/Icon.svelte';

  interface Hit {
    icon: IconName;
    label: string;
    meta?: string;
    route?: Route;
    action?: () => void;
  }

  let {
    open = $bindable(false),
    mode,
    projects,
    conversations,
    agents,
    onnavigate,
    onnewchat,
    onaddproject,
    oncommands,
    onload,
  }: {
    open?: boolean;
    mode: Mode;
    projects: Project[];
    conversations: Record<string, ConversationInfo[] | undefined>;
    agents: AgentEntry[];
    onnavigate: (route: Route) => void;
    onnewchat: () => void;
    onaddproject: () => void;
    oncommands: () => void;
    onload: (cwd: string) => Promise<void>;
  } = $props();

  let query = $state('');
  let selected = $state(0);
  let pending = $state(0);
  let field = $state<HTMLInputElement>();
  let list = $state<HTMLDivElement>();
  let restore: HTMLElement | null = null;
  let fanned = false;
  let pointer = { x: -1, y: -1 };

  const basename = (path: string) => path.split('/').filter(Boolean).at(-1) ?? path;

  const groups = $derived.by(() => {
    const term = query.trim().toLowerCase();
    const hit = (text: string) => !term || text.toLowerCase().includes(term);
    const out: { title: string; rows: Hit[] }[] = [];
    const push = (title: string, rows: Hit[]) => { if (rows.length) out.push({ title, rows }); };

    push('Commands', ([
      { icon: 'compose', label: 'New chat', action: onnewchat },
      { icon: 'folder', label: 'Add project folder…', action: onaddproject },
      { icon: 'plug', label: 'Integrations', route: { kind: 'integrations' } },
      ...(mode === 'agent' ? [{ icon: 'brain' as IconName, label: 'Memory', route: { kind: 'memory' } as Route }] : []),
      { icon: 'command', label: 'Slash commands', action: oncommands },
      { icon: 'settings', label: 'Settings', route: { kind: 'settings' } },
    ] satisfies Hit[]).filter(row => hit(row.label)));

    push('Chats', projects.flatMap(project =>
      (conversations[project.path] ?? [])
        .filter(item => hit(item.title ?? ''))
        .map(item => ({
          icon: 'chat' as IconName,
          label: item.title || 'Untitled',
          meta: basename(project.path),
          route: { kind: 'chat', cwd: project.path, conversation: item.conversation_id } as Route,
        }))));

    push('Projects', projects
      .filter(project => hit(project.path))
      .map(project => ({
        icon: 'folder' as IconName,
        label: basename(project.path),
        meta: project.path,
        route: { kind: 'chat', cwd: project.path, conversation: null, nonce: 0 } as Route,
      })));

    push('Agents', agents
      .filter(agent => hit(`${agent.slug} ${agent.display}`))
      .map(agent => ({
        icon: 'bot' as IconName,
        label: agent.display || agent.slug,
        route: { kind: 'talk', agent: agent.slug, conversation: null } as Route,
      })));

    return out;
  });

  const hits = $derived(groups.flatMap(group => group.rows));

  $effect(() => {
    if (selected >= hits.length) selected = Math.max(0, hits.length - 1);
  });

  $effect(() => {
    if (!open) return;
    restore = document.activeElement as HTMLElement | null;
    query = '';
    selected = 0;
    field?.focus();
    if (!fanned) {
      fanned = true;
      void fanOut();
    }
  });

  async function fanOut() {
    const missing = projects.filter(project => conversations[project.path] === undefined).map(project => project.path);
    if (!missing.length) return;
    pending = missing.length;
    const queue = [...missing];
    const worker = async () => {
      for (let next = queue.shift(); next !== undefined; next = queue.shift()) {
        await onload(next).catch(() => undefined);
        pending -= 1;
      }
    };
    await Promise.all(Array.from({ length: Math.min(4, queue.length) }, worker));
    pending = 0;
  }

  function close() {
    open = false;
    if (restore?.isConnected) restore.focus();
    restore = null;
  }

  function choose(hit: Hit | undefined) {
    if (!hit) return;
    close();
    if (hit.action) hit.action();
    else if (hit.route) onnavigate(hit.route);
  }

  function move(delta: number) {
    if (!hits.length) return;
    selected = Math.max(0, Math.min(selected + delta, hits.length - 1));
    queueMicrotask(() => list?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }));
  }

  function keys(event: KeyboardEvent) {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === 'Escape') { event.preventDefault(); close(); return; }
    if (event.key === 'ArrowDown') { event.preventDefault(); move(1); return; }
    if (event.key === 'ArrowUp') { event.preventDefault(); move(-1); return; }
    if (event.key === 'Enter') { event.preventDefault(); choose(hits[selected]); }
  }

  function track(event: MouseEvent, index: number) {
    if (event.clientX === pointer.x && event.clientY === pointer.y) return;
    pointer = { x: event.clientX, y: event.clientY };
    selected = index;
  }
</script>

{#if open}
  <div class="fixed inset-0 z-50 flex justify-center bg-black/40 pt-[15vh]">
    <button type="button" tabindex="-1" aria-label="Close search" class="absolute inset-0 cursor-default" onclick={close}></button>
    <div
      class="relative w-[520px] max-w-[92vw] self-start overflow-hidden rounded-xl border border-hairline bg-menu shadow-[0_16px_44px_#0000008c]"
      role="dialog"
      aria-modal="true"
      aria-label="Search"
    >
      <div class="flex items-center gap-[9px] border-b border-hairline px-[18px] py-3">
        <Icon name="search" class="text-fg-3" />
        <input
          bind:this={field}
          bind:value={query}
          class="min-w-0 flex-1 border-0 bg-transparent text-lg text-fg-1 outline-none placeholder:text-fg-3"
          aria-label="Search"
          autocomplete="off"
          spellcheck="false"
          placeholder="Search"
          onkeydown={keys}
        />
        {#if pending}
          <span class="text-xs text-fg-3">Searching {pending}…</span>
        {/if}
      </div>

      <div bind:this={list} class="max-h-[330px] overflow-y-auto px-2 pt-1.5 pb-2" role="listbox" aria-label="Results">
        {#if hits.length}
          {#each groups as group (group.title)}
            <div role="group" aria-label={group.title}>
              <p class="px-2.5 pt-2 pb-0.5 text-xs font-[450] text-fg-3" aria-hidden="true">{group.title}</p>
              {#each group.rows as row (row.label + (row.meta ?? ''))}
                {@const index = hits.indexOf(row)}
                <button
                  type="button"
                  role="option"
                  aria-selected={index === selected}
                  class="flex h-8 w-full items-center gap-[9px] rounded-md px-2.5 text-left text-fg-2 aria-selected:bg-fill-active aria-selected:text-fg-1"
                  onclick={() => choose(row)}
                  onmousemove={(event) => track(event, index)}
                >
                  <Icon name={row.icon} class="shrink-0 text-fg-3" />
                  <span class="min-w-0 flex-1 truncate">{row.label}</span>
                  {#if row.meta}<span class="shrink-0 text-sm text-fg-3">{row.meta}</span>{/if}
                </button>
              {/each}
            </div>
          {/each}
        {:else}
          <p class="px-2.5 py-6 text-center text-fg-3">No matches</p>
        {/if}
      </div>
    </div>
  </div>
{/if}

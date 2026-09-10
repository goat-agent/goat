<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-dialog';
  import type { AgentActivity, AgentChatItem, AgentConversationEntry, AgentEntry, AgentMessage, ConversationInfo, DaemonStatus2, Event, InputAttachment, Op, ResumeMode, ScheduleEntry } from '../shared/api';
  import { ipc, errorText, type AdminRequest, type CommandSpec, type ComputerStatus, type DesktopSessionInfo, type Project, type Workspace } from '../shared/ipc';
  import { applyEvent, createSession } from '../entities/session';
  import { routeKey, type Mode, type Route } from '../entities/routes';
  import { Navigation } from '../features/navigation.svelte';
  import { loadView, saveView } from '../features/view';
  import { dismissMenu } from '../features/menu';
  import SearchPalette from '../features/SearchPalette.svelte';
  import Icon from '../shared/Icon.svelte';
  import IconButton from '../shared/IconButton.svelte';
  import Markdown from '../shared/Markdown.svelte';
  import Sidebar from '../widgets/Sidebar.svelte';
  import WindowChrome from '../widgets/WindowChrome.svelte';
  import Transcript from '../widgets/Transcript.svelte';
  import Composer from '../widgets/Composer.svelte';
  import SessionPanel from '../widgets/SessionPanel.svelte';
  import AgentPanel from '../widgets/AgentPanel.svelte';

  const overlay = new URLSearchParams(window.location.search).get('overlay') === '1';
  const stored = overlay ? { collapsed: false, expanded: [] as string[], route: null } : loadView();
  const nav = new Navigation();

  let collapsed = $state(stored.collapsed);
  let expanded = $state<string[]>(stored.expanded);
  let searchOpen = $state(false);
  let nonce = 0;

  let projects = $state<Project[]>([]);
  let projectConversations = $state<Record<string, ConversationInfo[] | undefined>>({});
  let agents = $state<AgentEntry[]>([]);
  let agentConversations = $state<Record<string, AgentConversationEntry[] | undefined>>({});
  let messages = $state<AgentMessage[]>([]);
  let agentRevision = $state(0);
  let activity = $state<AgentActivity[]>([]);
  let schedules = $state<ScheduleEntry[]>([]);
  let typing = $state(false);
  let agentConnected = $state(false);
  let agentOpening = $state(false);
  let session = $state<DesktopSessionInfo | null>(null);
  let code = $state(createSession());
  let workspace = $state<Workspace | null>(null);
  let specs = $state<CommandSpec[]>([]);
  let connection = $state<'idle' | 'opening' | 'connected' | 'closed'>('idle');
  let presence = $state(1);
  let panel = $state('overview');
  let inspectorOpen = $state(true);
  let codeDraft = $state('');
  let codeAttachments = $state<InputAttachment[]>([]);
  let agentDraft = $state('');
  let agentAttachments = $state<InputAttachment[]>([]);
  let focusToken = $state(0);
  let paletteToken = $state(0);
  let daemon = $state<DaemonStatus2 | null>(null);
  let computer = $state<ComputerStatus | null>(null);
  let daemonError = $state('');
  let computerError = $state('');
  let errors = $state<{ id: number; message: string }[]>([]);
  let errorSerial = 0;
  let agentLoading = $state(false);
  let permissionPending = $state(false);
  let transcript = $state<HTMLDivElement>();
  let follow = $state(true);
  let unseen = $state(false);
  let disposed = false;
  let sessionGeneration = 0;
  let sessionTransition: Promise<void> = Promise.resolve();
  let sessionListeners: UnlistenFn[] = [];
  let agentListeners: UnlistenFn[] = [];
  let activeAgent = $state('');
  let activeConversation = $state<string | null>(null);
  let agentGeneration = 0;
  let agentTransition: Promise<void> = Promise.resolve();
  let pendingAgentUpdates: Record<string, string> = {};
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let appliedKey = '';

  const compact = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });
  const route = $derived(nav.current);
  const mode = $derived<Mode>(nav.mode);
  const screen = $derived(route.kind === 'integrations' || route.kind === 'memory' || route.kind === 'settings');
  const currentAgent = $derived(agents.find(agent => agent.slug === (route.kind === 'talk' ? route.agent : activeAgent)));
  const busyConversation = $derived(code.busy ? code.conversation : null);
  const title = $derived(session
    ? projectConversations[session.cwd]?.find(item => item.conversation_id === code.conversation)?.title || 'New session'
    : 'Your next good idea');

  const screenTitle: Record<string, string> = {
    integrations: 'Integrations',
    memory: 'Memory',
    settings: 'Settings',
  };

  function showError(message: string) {
    if (!message || disposed || errors.some(error => error.message === message)) return;
    errors.push({ id: ++errorSerial, message });
  }
  function report(promise: Promise<unknown>) {
    void promise.catch(error => showError(errorText(error)));
  }
  async function refreshHealth() {
    await Promise.all([
      ipc.daemon().then(value => { if (!disposed) { daemon = value; daemonError = ''; } }).catch(error => { if (!disposed) { daemon = null; daemonError = errorText(error); } }),
      ipc.computer().then(value => { if (!disposed) { computer = value; computerError = ''; } }).catch(error => { if (!disposed) computerError = errorText(error); }),
    ]);
  }
  async function loadProjects() {
    projects = await ipc.projects();
  }
  async function loadAgents() {
    agentLoading = true;
    try { agents = (await ipc.agents()).agents; }
    finally { agentLoading = false; }
  }
  async function loadConversations(cwd: string) {
    const list = await ipc.conversations(cwd);
    if (!disposed) projectConversations[cwd] = list;
  }
  async function loadAgentConversations(slug: string) {
    const list = await ipc.agentConversations(slug);
    if (!disposed) agentConversations[slug] = list.conversations;
  }
  function expand(key: string) {
    if (expanded.includes(key)) return;
    expanded.push(key);
    if (key.startsWith('/')) {
      if (projectConversations[key] === undefined) report(loadConversations(key));
    } else if (agentConversations[key] === undefined) {
      report(loadAgentConversations(key));
    }
  }
  function toggleExpanded(key: string) {
    if (expanded.includes(key)) expanded = expanded.filter(value => value !== key);
    else expand(key);
  }
  async function addProject() {
    const path = await open({ directory: true, multiple: false, title: 'Add a project' });
    if (typeof path !== 'string') return;
    projects = await ipc.addProject(path);
    expand(path);
    await loadConversations(path);
    nav.go({ kind: 'chat', cwd: path, conversation: null, nonce: ++nonce });
  }
  async function removeProject(path: string) {
    projects = await ipc.removeProject(path);
    expanded = expanded.filter(value => value !== path);
    delete projectConversations[path];
    nav.prune(entry => entry.kind === 'chat' && entry.cwd === path, fallbackRoute());
  }

  function fallbackRoute(): Route {
    const cwd = projects[0]?.path;
    return cwd ? { kind: 'chat', cwd, conversation: null, nonce: ++nonce } : { kind: 'welcome' };
  }

  function navigate(target: Route) {
    if (target.kind === 'chat') expand(target.cwd);
    if (target.kind === 'talk') expand(target.agent);
    nav.go(target);
  }
  function switchMode(next: Mode) {
    if (next === mode) return;
    const remembered = nav.latest(next);
    if (remembered) { navigate(remembered); return; }
    if (next === 'code') {
      const cwd = projects[0]?.path;
      if (cwd) navigate({ kind: 'chat', cwd, conversation: null, nonce: ++nonce });
      else report(addProject());
    } else {
      const slug = agents[0]?.slug;
      if (slug) navigate({ kind: 'talk', agent: slug, conversation: null });
    }
  }
  function newChat(owner?: string) {
    if (mode === 'code') {
      const cwd = owner ?? (route.kind === 'chat' ? route.cwd : projects[0]?.path);
      if (!cwd) { report(addProject()); return; }
      navigate({ kind: 'chat', cwd, conversation: null, nonce: ++nonce });
    } else {
      const slug = owner ?? (route.kind === 'talk' ? route.agent : agents[0]?.slug);
      if (!slug) return;
      agentDraft = ''; agentAttachments = [];
      navigate({ kind: 'talk', agent: slug, conversation: null, nonce: ++nonce });
      focusToken++;
    }
  }
  function toggleSidebar() {
    collapsed = !collapsed;
  }

  async function applyRoute(target: Route) {
    if (target.kind === 'chat') {
      if (session?.cwd === target.cwd && code.conversation === target.conversation && connection !== 'closed') return;
      await openCode(target.cwd, target.conversation === null ? { type: 'New' } : { type: 'Conversation', conversation_id: target.conversation });
      return;
    }
    if (target.kind === 'talk') {
      if (activeAgent === target.agent && activeConversation === target.conversation && agentConnected) return;
      if (!agents.length) await loadAgents();
      await selectAgent(target.agent, target.conversation);
    }
  }

  function closeCode(): Promise<void> {
    sessionGeneration++;
    sessionTransition = sessionTransition.catch(error => showError(errorText(error))).then(async () => {
      const previous = session;
      sessionListeners.splice(0).forEach(unlisten => unlisten());
      session = null; connection = 'idle';
      if (previous) await ipc.closeSession(previous.id);
    });
    return sessionTransition;
  }
  function openCode(cwd: string, resume: ResumeMode): Promise<void> {
    const generation = ++sessionGeneration;
    connection = 'opening';
    sessionTransition = sessionTransition.catch(error => showError(errorText(error))).then(async () => {
      if (generation === sessionGeneration && !disposed) await attachCode(cwd, resume, generation);
    });
    return sessionTransition;
  }
  async function attachCode(cwd: string, resume: ResumeMode, generation: number) {
    const previous = session;
    sessionListeners.splice(0).forEach(unlisten => unlisten());
    session = null; connection = 'opening'; panel = 'overview';
    expand(cwd);
    try {
      if (previous) await ipc.closeSession(previous.id);
      if (generation !== sessionGeneration || disposed) return;
      const opened = await ipc.openSession(cwd, resume);
      if (generation !== sessionGeneration || disposed) { await ipc.closeSession(opened.id); return; }
      session = opened; code = createSession(); codeDraft = ''; codeAttachments = []; specs = []; workspace = null; presence = 1; follow = true;
      const view = code;
      const listeners: UnlistenFn[] = [];
      try {
        listeners.push(await listen<Event>(`session:${opened.id}`, event => {
          if (generation !== sessionGeneration || disposed) return;
          try {
            const sequence = view.draft.sequence;
            applyEvent(view, event.payload);
            if (view.draft.sequence !== sequence) {
              codeDraft = event.payload.type === 'ConversationRewound' ? view.draft.text : [view.draft.text, codeDraft].filter(Boolean).join('\n');
              codeAttachments = event.payload.type === 'ConversationRewound' ? view.draft.attachments : [...view.draft.attachments, ...codeAttachments];
              focusToken++;
            }
            if (event.payload.type === 'ConversationBound') {
              const bound: Route = { kind: 'chat', cwd, conversation: event.payload.conversation_id };
              if (nav.current.kind === 'chat' && nav.current.cwd === cwd) {
                nav.replace(bound);
                appliedKey = routeKey(bound);
              }
            }
            if (event.payload.type === 'SkillsChanged') report(ipc.specs(opened.id).then(value => { if (generation === sessionGeneration) specs = value; }));
            if (['TaskDone', 'ConversationBound', 'ConversationRestored', 'UserMessage'].includes(event.payload.type)) {
              clearTimeout(refreshTimer);
              refreshTimer = setTimeout(() => report(loadConversations(cwd)), 350);
            }
            if (event.payload.type === 'ToolDone' && event.payload.outcome.git) report(ipc.workspace(cwd).then(value => { if (generation === sessionGeneration) workspace = value; }));
          } catch (error) { showError(errorText(error)); }
        }));
        listeners.push(await listen<number>(`session:${opened.id}:presence`, event => { if (generation === sessionGeneration) presence = event.payload; }));
        listeners.push(await listen<unknown>(`session:${opened.id}:closed`, event => {
          if (generation !== sessionGeneration || disposed) return;
          connection = 'closed'; view.busy = false; view.active = null;
          showError(typeof event.payload === 'string' && event.payload ? event.payload : 'Session disconnected. Reconnect to continue where you left off.');
        }));
        if (generation !== sessionGeneration || disposed) {
          listeners.forEach(unlisten => unlisten());
          session = null;
          await ipc.closeSession(opened.id);
          return;
        }
        sessionListeners = listeners;
        await ipc.listenSession(opened.id);
        if (generation !== sessionGeneration || disposed) return;
        connection = 'connected'; focusToken++;
        report(ipc.specs(opened.id).then(value => { if (generation === sessionGeneration) specs = value; }));
        report(ipc.workspace(cwd).then(value => { if (generation === sessionGeneration) workspace = value; }));
      } catch (error) {
        listeners.forEach(unlisten => unlisten());
        await ipc.closeSession(opened.id).catch(closeError => showError(errorText(closeError)));
        throw error;
      }
    } catch (error) {
      if (generation === sessionGeneration) connection = session ? 'closed' : 'idle';
      throw error;
    }
  }
  async function operation(op: Op) {
    if (!session || connection !== 'connected') throw new Error('Open a connected code session first.');
    await ipc.op(session.id, op);
    if (op.type === 'ResolvePlan') code.plan = null;
  }
  async function admin(request: AdminRequest) {
    if (!session || connection !== 'connected') throw new Error('Open a connected code session first.');
    await ipc.admin(session.id, request);
  }
  async function openPanel(target: string) {
    panel = target; inspectorOpen = true;
    if (target === 'resume') await operation({ type: 'ListConversations' });
    else if (target === 'rewind') await operation({ type: 'ListRewindPoints' });
    else if (target === 'config') await operation({ type: 'RefreshAccounts' });
  }
  async function interrupt() {
    if (code.active) await operation({ type: 'Interrupt', id: code.active });
  }
  async function sendCode(text: string, attachments: InputAttachment[]) {
    if (!session || connection !== 'connected') throw new Error('Open a connected code session first.');
    const value = text.trimStart();
    if ((value.startsWith('/') || value.startsWith('!')) && attachments.length) throw new Error('Image attachments can only be sent with a code message, not a slash command or shell command.');
    if (value.startsWith('/')) {
      const result = await ipc.command(session.id, value);
      if (result.kind === 'show') await openPanel(result.screen);
      else if (result.kind === 'notice') code.notices.push({ key: ++code.serial, level: result.level, text: result.text });
      else if (result.kind === 'quit') await closeCode();
      return;
    }
    if (value.startsWith('!')) {
      const command = value.slice(1).trim();
      if (!command) throw new Error('Enter a shell command after !.');
      const key = ++code.serial;
      code.rows.push({ kind: 'shell', key, task: `local-${key}`, command, output: '', done: false });
      try { await operation({ type: 'SubmitShell', id: '0', command }); }
      catch (error) { code.rows = code.rows.filter(row => row.key !== key); throw error; }
      return;
    }
    const temporary = `local-${++code.serial}`;
    const view = code;
    view.queued.push({ id: temporary, text, attachments });
    try {
      const id = await ipc.submit(session.id, text, attachments);
      const queued = view.queued.find(message => message.id === temporary);
      if (queued) queued.id = String(id);
    } catch (error) { view.queued = view.queued.filter(message => message.id !== temporary); throw error; }
  }
  async function stopAgent() {
    const slug = activeAgent;
    activeAgent = ''; activeConversation = null; agentConnected = false; typing = false;
    agentListeners.splice(0).forEach(unlisten => unlisten());
    if (slug) {
      const closed = await Promise.allSettled([ipc.closeAgent(slug), ipc.closeActivity(slug)]);
      for (const result of closed) if (result.status === 'rejected') showError(errorText(result.reason));
    }
  }
  function selectAgent(slug: string, conversation: string | null): Promise<void> {
    const generation = ++agentGeneration;
    agentOpening = true;
    agentTransition = agentTransition.catch(error => showError(errorText(error))).then(async () => {
      await stopAgent();
      if (generation !== agentGeneration || disposed) return;
      messages = []; activity = []; schedules = []; pendingAgentUpdates = {}; follow = true;
      agentRevision++;
      activeAgent = slug; activeConversation = conversation;
      try {
        agentListeners.push(await listen<AgentChatItem>(`agent:${slug}`, event => {
          if (generation !== agentGeneration || disposed) return;
          const item = event.payload;
          if (item.t === 'snapshot') messages = item.messages.map(message => ({ ...message, text: pendingAgentUpdates[message.id] ?? message.text }));
          else if (item.t === 'message') {
            const message = { ...item.message, text: pendingAgentUpdates[item.message.id] ?? item.message.text };
            delete pendingAgentUpdates[message.id];
            const index = messages.findIndex(existing => existing.id === message.id);
            if (index < 0) messages.push(message); else messages[index] = message;
          } else if (item.t === 'update') {
            const index = messages.findIndex(message => message.id === item.id);
            if (index < 0) pendingAgentUpdates[item.id] = item.text;
            else messages[index].text = item.text;
          } else typing = item.active;
          agentRevision++;
        }));
        agentListeners.push(await listen<AgentActivity>(`agent:${slug}:activity`, event => {
          if (generation === agentGeneration && !disposed) activity = [event.payload, ...activity].slice(0, 200);
        }));
        agentListeners.push(await listen<string | null>(`agent:${slug}:closed`, event => {
          if (generation !== agentGeneration || disposed) return;
          agentConnected = false; typing = false;
          showError(event.payload || 'Agent chat disconnected. Select the agent again to reconnect.');
        }));
        agentListeners.push(await listen<string | null>(`agent:${slug}:activity:closed`, event => {
          if (generation === agentGeneration && !disposed) showError(event.payload || 'Agent activity disconnected. Select the agent again to reconnect.');
        }));
        if (generation !== agentGeneration || disposed) { await stopAgent(); return; }
        await Promise.all([ipc.openAgent(slug, conversation), ipc.openActivity(slug)]);
        if (generation !== agentGeneration || disposed) { await stopAgent(); return; }
        agentConnected = true;
        report(refreshSchedules(slug));
        focusToken++;
      } catch (error) {
        await stopAgent();
        throw error;
      } finally { if (generation === agentGeneration) agentOpening = false; }
    });
    return agentTransition;
  }
  async function refreshSchedules(slug: string) {
    const result = await ipc.schedules(slug);
    if (activeAgent === slug) schedules = result.schedules;
  }
  async function sendAgent(text: string, attachments: InputAttachment[]) {
    if (attachments.length) throw new Error('images go to code sessions for now');
    if (!activeAgent || !agentConnected) throw new Error('Select a connected agent first.');
    const slug = activeAgent;
    let conversation = activeConversation;
    if (conversation === null) {
      conversation = crypto.randomUUID();
      const bound: Route = { kind: 'talk', agent: slug, conversation };
      activeConversation = conversation;
      nav.replace(bound);
      appliedKey = routeKey(bound);
    }
    await ipc.sendAgent(slug, conversation, text);
    report(loadAgentConversations(slug));
  }
  async function showCommands() {
    if (mode !== 'code') switchMode('code');
    if (!session) newChat();
    if (session) { paletteToken++; focusToken++; }
  }
  function keyboard(event: KeyboardEvent) {
    if (overlay || event.defaultPrevented || event.isComposing || event.keyCode === 229) return;
    if (searchOpen) return;
    const command = (event.metaKey || event.ctrlKey) && !event.altKey;
    if (command && !event.repeat) {
      const key = event.key.toLowerCase();
      if (key === 'k') { event.preventDefault(); searchOpen = true; return; }
      if (key === 'b') { event.preventDefault(); toggleSidebar(); return; }
      if (key === 'p' && event.shiftKey) { event.preventDefault(); report(showCommands()); return; }
      if (key === 'n') { event.preventDefault(); newChat(); return; }
      if (event.key === '1') { event.preventDefault(); switchMode('code'); return; }
      if (event.key === '2') { event.preventDefault(); switchMode('agent'); return; }
      if (event.key === '[') { event.preventDefault(); nav.back(); return; }
      if (event.key === ']') { event.preventDefault(); nav.forward(); return; }
    }
    if (event.key === 'Escape' && code.busy && mode === 'code') { event.preventDefault(); report(interrupt()); }
  }
  async function requestPermission(kind: 'accessibility' | 'screen' | 'resume') {
    permissionPending = true;
    try { if (kind === 'resume') await ipc.resumeComputer(); else await ipc.permission(kind); await refreshHealth(); }
    finally { permissionPending = false; }
  }
  function scroll() {
    follow = !transcript || transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight < 100;
    if (follow) unseen = false;
  }
  async function latest() {
    follow = true; unseen = false;
    await tick(); transcript?.scrollTo({ top: transcript.scrollHeight, behavior: 'smooth' });
  }

  $effect(() => {
    if (overlay) return;
    const key = routeKey(nav.current);
    if (key === appliedKey) return;
    appliedKey = key;
    report(applyRoute(nav.current));
  });
  $effect(() => {
    if (overlay) return;
    saveView({ collapsed, expanded: [...expanded], route: nav.current });
  });
  $effect(() => {
    code.revision; agentRevision; typing; mode;
    if (follow) void tick().then(() => { if (transcript) transcript.scrollTop = transcript.scrollHeight; });
    else unseen = true;
  });
  onMount(() => {
    if (overlay) { document.documentElement.classList.add('overlay-surface'); return; }
    report(Promise.all([loadProjects(), loadAgents()]).then(() => {
      for (const cwd of expanded) if (projects.some(project => project.path === cwd)) report(loadConversations(cwd));
      const restored = stored.route;
      const usable = restored
        && ((restored.kind === 'chat' && projects.some(project => project.path === restored.cwd))
          || (restored.kind === 'talk' && agents.some(agent => agent.slug === restored.agent))
          || restored.kind === 'integrations' || restored.kind === 'memory' || restored.kind === 'settings');
      if (usable && restored) nav.go(restored);
      else if (projects.length) nav.go({ kind: 'chat', cwd: projects[0].path, conversation: null, nonce: ++nonce });
    }));
    report(refreshHealth());
    const health = setInterval(() => report(refreshHealth()), 5000);
    let haltedOff: UnlistenFn | undefined;
    report(listen('computer:halted', () => report(refreshHealth())).then(unlisten => { if (disposed) unlisten(); else haltedOff = unlisten; }));
    return () => {
      disposed = true; sessionGeneration++; agentGeneration++;
      clearInterval(health); clearTimeout(refreshTimer); haltedOff?.();
      sessionListeners.splice(0).forEach(unlisten => unlisten());
      agentListeners.splice(0).forEach(unlisten => unlisten());
      void dismissMenu();
      if (session) void ipc.closeSession(session.id).catch(() => {});
      if (activeAgent) { void ipc.closeAgent(activeAgent).catch(() => {}); void ipc.closeActivity(activeAgent).catch(() => {}); }
    };
  });
</script>

<svelte:window onkeydown={keyboard} />

{#if overlay}
  <div class="computer-overlay"><div>goat is driving <span>·</span> ⌘⇧Esc to stop</div></div>
{:else}
  <div
    class={[
      'grid h-full w-full grid-rows-[minmax(0,1fr)_30px]',
      'grid-cols-[var(--shell-sidebar)_minmax(0,1fr)]',
      'transition-[grid-template-columns] duration-[190ms] ease-fold motion-reduce:transition-none',
      inspectorOpen && !screen && 'md:grid-cols-[var(--shell-sidebar)_minmax(0,1fr)_286px] xl:grid-cols-[var(--shell-sidebar)_minmax(0,1fr)_310px]',
      collapsed ? '[--shell-sidebar:0px]' : '[--shell-sidebar:260px] max-lg:[--shell-sidebar:236px] max-sm:[--shell-sidebar:220px]',
    ]}
  >
    <Sidebar
      {mode}
      {route}
      {collapsed}
      {projects}
      {agents}
      {expanded}
      {busyConversation}
      agentConversations={agentConversations}
      canBack={nav.canBack}
      canForward={nav.canForward}
      conversations={projectConversations}
      onnavigate={navigate}
      onmode={switchMode}
      onnewchat={newChat}
      ontoggleexpanded={toggleExpanded}
      onaddproject={() => report(addProject())}
      onremoveproject={(path) => report(removeProject(path))}
      onsearch={() => (searchOpen = true)}
      ontoggle={toggleSidebar}
      onback={() => nav.back()}
      onforward={() => nav.forward()}
      onerror={showError}
    />

    <main class="relative col-start-2 row-start-1 flex min-h-0 min-w-0 flex-col bg-canvas">
      <header
        class={[
          'flex h-[81px] shrink-0 items-center justify-between gap-3 select-none',
          'border-b border-hairline pr-[28px] max-lg:pr-[18px]',
          collapsed ? 'pl-0' : 'pl-[28px] max-lg:pl-[21px] max-sm:pl-[18px]',
        ]}
        data-tauri-drag-region
      >
        {#if collapsed}
          <WindowChrome
            {collapsed}
            canBack={nav.canBack}
            canForward={nav.canForward}
            ontoggle={toggleSidebar}
            onback={() => nav.back()}
            onforward={() => nav.forward()}
          />
        {/if}
        <div class="header-title" data-tauri-drag-region>
          {#if screen}
            <h1 data-tauri-drag-region>{screenTitle[route.kind]}</h1>
          {:else if mode === 'code'}
            <div class="header-breadcrumb" data-tauri-drag-region>
              {#if workspace?.branch}
                <Icon name="git" size={13} />{workspace.repo?.split('/').at(-1)}<span>/</span><strong>{workspace.branch}</strong>
              {:else}
                <span>{session ? session.cwd.split('/').filter(Boolean).at(-1) : 'Code workspace'}</span>
              {/if}
              {#if workspace?.pr}<span class="pr-badge" class:merged={workspace.pr.state.toLowerCase() === 'merged'}>PR #{workspace.pr.number} · {workspace.pr.state.toLowerCase()}</span>{/if}
            </div>
            <h1 data-tauri-drag-region>{title}</h1>
          {:else}
            <div class="header-breadcrumb" data-tauri-drag-region>
              <span class="status-dot" class:offline={!agentConnected}></span>{agentConnected ? 'Connected' : agentOpening ? 'Connecting…' : 'Agent workspace'}
            </div>
            <h1 data-tauri-drag-region>{currentAgent?.display || currentAgent?.slug || 'A little more possible'}</h1>
          {/if}
        </div>
        <div class="header-actions">
          {#if !screen && mode === 'code' && session}
            {#if connection === 'closed'}
              <button class="secondary" onclick={() => report(openCode(session!.cwd, code.conversation ? { type: 'Conversation', conversation_id: code.conversation } : { type: 'Latest' }))}><Icon name="refresh" size={13} />Reconnect</button>
            {:else}
              <button class="model-button" onclick={() => report(openPanel('model'))}><Icon name="cpu" size={13} /><span>{code.model?.model ?? 'Choose model'}</span><Icon name="down" size={11} /></button>
              {#if code.contextWindow}
                <button class="context-meter" title={`${compact.format(code.contextTokens ?? 0)} of ${compact.format(code.contextWindow)} context tokens`} aria-label="View context usage" onclick={() => report(openPanel('usage'))}>
                  <svg viewBox="0 0 28 28" aria-hidden="true"><circle cx="14" cy="14" r="10" /><circle class="meter-value" cx="14" cy="14" r="10" pathLength="100" stroke-dasharray={`${Math.min(100, (code.contextTokens ?? 0) / code.contextWindow * 100)} 100`} /></svg>
                </button>
              {/if}
            {/if}
          {/if}
          {#if !screen}
            <IconButton icon="panel" label="Toggle inspector" onclick={() => (inspectorOpen = !inspectorOpen)} class={inspectorOpen ? 'text-fg-1' : undefined} />
          {/if}
        </div>
      </header>

      {#if errors.length || daemonError || computerError}
        <div class="app-errors" role="alert">
          {#if daemonError}<div><span><strong>Daemon unavailable</strong>{daemonError}</span><button class="icon-button" aria-label="Retry daemon connection" onclick={() => report(refreshHealth())}><Icon name="refresh" size={14} /></button></div>{/if}
          {#if computerError}<div><span><strong>Computer provider unavailable</strong>{computerError}</span><button class="icon-button" aria-label="Retry computer status" onclick={() => report(refreshHealth())}><Icon name="refresh" size={14} /></button></div>{/if}
          {#each errors as error (error.id)}<div><span>{error.message}</span><button class="icon-button" aria-label="Dismiss error" onclick={() => errors = errors.filter(item => item.id !== error.id)}><Icon name="close" size={14} /></button></div>{/each}
        </div>
      {/if}

      <div class="transcript-scroll" bind:this={transcript} onscroll={scroll}>
        {#if screen}
          <div class="welcome">
            <span class="welcome-glyph"><Icon name={route.kind === 'memory' ? 'brain' : route.kind === 'settings' ? 'settings' : 'plug'} size={30} /></span>
            <h2>{screenTitle[route.kind]}</h2>
            <p>Not wired up yet. This screen has its route and its place in the shell.</p>
          </div>
        {:else if mode === 'code'}
          {#if connection === 'opening'}
            <div class="welcome"><span class="spinner large-spinner"></span><h2>Opening your workspace</h2><p>Connecting to the local daemon…</p></div>
          {:else if session}
            <Transcript state={code} onop={operation} onerror={showError} />
            {#if !code.rows.length && !code.busy}
              <div class="session-welcome"><span class="welcome-glyph">g</span><h2>What are we building?</h2><p>Explore an idea. Untangle a bug. Make something work.</p><div class="welcome-hints"><span><Icon name="code" size={14} /> Code with context</span><span><Icon name="shield" size={14} /> On your machine</span></div></div>
            {/if}
          {:else}
            <div class="welcome"><span class="welcome-glyph">g</span><div class="eyebrow">Your ideas, in motion</div><h2>A clear space<br />for your next good idea.</h2><p>Open a project and work alongside goat.<br />From the first question to the final commit.</p><button class="primary" onclick={() => (projects.length ? newChat() : report(addProject()))}><Icon name="plus" size={16} />{projects.length ? 'Start a session' : 'Open a project'}</button><span class="welcome-shortcut">or press ⌘N</span></div>
          {/if}
        {:else if currentAgent}
          <div class="transcript-content agent-transcript">
            {#each messages as message (message.id)}
              <article class="transcript-message" class:user-message={!message.outgoing} class:assistant-message={message.outgoing}>
                <div class="message-label"><span class="avatar" class:goat-avatar={message.outgoing} class:user-avatar={!message.outgoing}>{message.outgoing ? (currentAgent.display || currentAgent.slug).slice(0, 1).toUpperCase() : 'Y'}</span>{message.outgoing ? currentAgent.display || currentAgent.slug : 'You'}<time datetime={message.ts}>{new Date(message.ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time></div>
                {#if message.outgoing}<Markdown text={message.text} onerror={showError} />{:else}<div class="user-text">{message.text}</div>{/if}
              </article>
            {/each}
            {#if typing}<div class="working-row"><span class="typing-indicator"><i></i><i></i><i></i></span>{currentAgent.display || currentAgent.slug} is thinking</div>{/if}
          </div>
          {#if !messages.length && !typing}
            <div class="session-welcome"><span class="agent-avatar large-avatar">{(currentAgent.display || currentAgent.slug).slice(0, 1).toUpperCase()}</span><h2>{agentOpening ? 'Opening your conversation…' : `Meet ${currentAgent.display || currentAgent.slug}`}</h2><p>{agentOpening ? 'Loading messages from your daemon.' : 'Delegate a task, ask a question, or plan what comes next.'}</p></div>
          {/if}
        {:else}
          <div class="welcome"><span class="welcome-glyph"><Icon name="agent" size={36} /></span><div class="eyebrow">A capable companion</div><h2>Good work<br />doesn't have to wait.</h2><p>Select an agent to start a conversation.<br />Its routines, connections, and activity live here.</p>{#if !agents.length}<button class="secondary" disabled={agentLoading} onclick={() => report(loadAgents())}><Icon name="refresh" size={14} />Refresh agents</button>{/if}</div>
        {/if}
      </div>

      {#if unseen}<button class="jump-latest" onclick={() => report(latest())}>Jump to latest<Icon name="down" size={13} /></button>{/if}
      {#if !screen && mode === 'code' && code.notices.length}
        <div class="toast-stack" aria-live="polite">{#each code.notices as notice (notice.key)}<div class:error-toast={notice.level === 'error'} class="toast"><Icon name={notice.level === 'error' ? 'close' : notice.level === 'success' ? 'check' : 'agent'} size={14} /><span>{notice.text}</span><button class="icon-button" aria-label="Dismiss notification" onclick={() => code.notices = code.notices.filter(item => item.key !== notice.key)}><Icon name="close" size={12} /></button></div>{/each}</div>
      {/if}
      {#if !screen && mode === 'code' && code.queued.length}
        <div class="queued-messages">{#each code.queued as queued (queued.id)}<div><span class="pulse-dot"></span><span>Queued</span><p>{queued.text || `${queued.attachments.length} image attachments`}</p></div>{/each}</div>
      {/if}
      {#if !screen}
        {#if mode === 'code'}
          {#key session?.id}<Composer bind:value={codeDraft} bind:attachments={codeAttachments} {mode} busy={code.busy} disabled={connection !== 'connected'} plan={code.mode === 'plan'} {specs} files={code.files} {focusToken} {paletteToken} onsubmit={sendCode} oninterrupt={interrupt} onop={operation} onerror={showError} />{/key}
        {:else}
          {#key `${activeAgent}:${activeConversation}`}<Composer bind:value={agentDraft} bind:attachments={agentAttachments} {mode} busy={typing} disabled={!agentConnected} {focusToken} onsubmit={sendAgent} oninterrupt={interrupt} onop={operation} onerror={showError} />{/key}
        {/if}
      {/if}
    </main>

    {#if inspectorOpen && !screen}
      <aside class="inspector">
        {#if mode === 'code' && session}
          <SessionPanel screen={panel} state={code} {session} {workspace} {daemon} {presence} {specs} onop={operation} onadmin={admin} onpanel={target => report(openPanel(target))} onclose={() => panel = 'overview'} onerror={showError} />
        {:else if mode === 'agent' && currentAgent}
          <AgentPanel agent={currentAgent} {schedules} {activity} onrefresh={() => refreshSchedules(activeAgent)} onerror={showError} />
        {:else}
          <div class="inspector-heading"><span>{mode === 'code' ? 'Your workspace' : 'Agent details'}</span><Icon name="panel" size={15} /></div>
          <div class="inspector-body"><div class="inspector-intro"><span class="intro-line"></span><h3>Everything in context.</h3><p>{mode === 'code' ? 'Models, usage, and workspace details stay close to the work.' : 'Routines, connections, and live activity, all in one place.'}</p></div><div class="local-info"><Icon name="shield" size={19} /><h3>Local by design</h3><p>Your desktop connects directly to the daemon on this machine.</p></div></div>
        {/if}
      </aside>
    {/if}

    <footer class="statusbar">
      <div class="daemon-status" title={daemonError || (daemon ? `goat ${daemon.version} · PID ${daemon.pid} · ${daemon.sessions} sessions` : 'Connecting to daemon')}><span class="status-dot" class:offline={!daemon?.ready}></span>{daemon?.ready ? 'Daemon connected' : daemonError ? 'Daemon unavailable' : 'Connecting…'}{#if daemon}<span class="status-version">v{daemon.version}</span>{/if}</div>
      <div class="computer-status">{#if computer}<Icon name="shield" size={12} />{#if computer.halted}<span class="stopped-label">Stopped</span><button disabled={permissionPending} onclick={() => report(requestPermission('resume'))}>Resume computer use</button>{:else if !computer.accessibility || !computer.screen}<span>Computer use</span>{#if !computer.accessibility}<button disabled={permissionPending} onclick={() => report(requestPermission('accessibility'))}>Grant Accessibility</button>{/if}{#if !computer.screen}<button disabled={permissionPending} onclick={() => report(requestPermission('screen'))}>Grant Screen Recording</button>{/if}{:else}<span class:driving={computer.busy}>{computer.busy ? 'goat is driving' : computer.advertised ? 'Computer ready' : 'Connecting computer…'}</span><kbd>⌘⇧Esc to stop</kbd>{/if}{:else}<span>Computer {computerError ? 'unavailable' : 'connecting…'}</span>{/if}</div>
    </footer>
  </div>

  <SearchPalette
    bind:open={searchOpen}
    {mode}
    {projects}
    {agents}
    conversations={projectConversations}
    onnavigate={navigate}
    onnewchat={() => newChat()}
    onaddproject={() => report(addProject())}
    oncommands={() => report(showCommands())}
    onload={loadConversations}
  />
{/if}

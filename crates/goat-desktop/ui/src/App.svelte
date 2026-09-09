<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-dialog';
  import type { AgentActivity, AgentChatItem, AgentEntry, AgentMessage, ConversationInfo, DaemonStatus2, Event, InputAttachment, Op, ResumeMode, ScheduleEntry } from './lib/api';
  import { ipc, errorText, type AdminRequest, type CommandSpec, type ComputerStatus, type DesktopSessionInfo, type Project, type Workspace } from './lib/ipc';
  import { applyEvent, createSession } from './lib/session';
  import Icon from './components/Icon.svelte';
  import Markdown from './components/Markdown.svelte';
  import Transcript from './components/Transcript.svelte';
  import Composer from './components/Composer.svelte';
  import SessionPanel from './components/SessionPanel.svelte';
  import AgentPanel from './components/AgentPanel.svelte';

  const overlay = new URLSearchParams(window.location.search).get('overlay') === '1';
  let mode = $state<'code' | 'agent'>('code');
  let switcher = $state(false);
  let search = $state('');
  let projects = $state<Project[]>([]);
  let projectConversations = $state<Record<string, ConversationInfo[]>>({});
  let expanded = $state<string[]>([]);
  let selectedProject = $state('');
  let agents = $state<AgentEntry[]>([]);
  let selectedAgent = $state('');
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
  let projectLoading = $state(false);
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
  let activeAgent = '';
  let agentGeneration = 0;
  let agentTransition: Promise<void> = Promise.resolve();
  let pendingAgentUpdates: Record<string, string> = {};
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  const compact = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });
  const currentAgent = $derived(agents.find(agent => agent.slug === selectedAgent));
  const filteredProjects = $derived(projects.filter(project => project.path.toLowerCase().includes(search.toLowerCase()) || projectConversations[project.path]?.some(conversation => conversation.title?.toLowerCase().includes(search.toLowerCase()))));
  const filteredAgents = $derived(agents.filter(agent => `${agent.slug} ${agent.display}`.toLowerCase().includes(search.toLowerCase())));
  const title = $derived(session ? projectConversations[session.cwd]?.find(conversation => conversation.conversation_id === code.conversation)?.title || 'New session' : 'Your next good idea');

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
    projectLoading = true;
    try {
      projects = await ipc.projects();
      if (!selectedProject && projects.length) { selectedProject = projects[0].path; expanded = [selectedProject]; }
      if (selectedProject) await loadConversations(selectedProject);
    } finally { projectLoading = false; }
  }
  async function loadAgents() {
    agentLoading = true;
    try { agents = (await ipc.agents()).agents; }
    finally { agentLoading = false; }
  }
  async function loadConversations(cwd: string) {
    const conversations = await ipc.conversations(cwd);
    if (!disposed) projectConversations[cwd] = conversations;
  }
  async function addProject() {
    const path = await open({ directory: true, multiple: false, title: 'Add a project' });
    if (typeof path !== 'string') return;
    projects = await ipc.addProject(path);
    selectedProject = path;
    if (!expanded.includes(path)) expanded.push(path);
    await loadConversations(path);
    await openCode(path, { type: 'New' });
  }
  async function removeProject(path: string) {
    projects = await ipc.removeProject(path);
    expanded = expanded.filter(value => value !== path);
    if (selectedProject === path) selectedProject = projects[0]?.path ?? '';
    delete projectConversations[path];
  }
  async function toggleProject(path: string) {
    selectedProject = path;
    if (expanded.includes(path)) expanded = expanded.filter(value => value !== path);
    else { expanded.push(path); await loadConversations(path); }
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
    session = null; connection = 'opening'; selectedProject = cwd; panel = 'overview';
    if (!expanded.includes(cwd)) expanded.push(cwd);
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
  async function openPanel(screen: string) {
    panel = screen; inspectorOpen = true;
    if (screen === 'resume') await operation({ type: 'ListConversations' });
    else if (screen === 'rewind') await operation({ type: 'ListRewindPoints' });
    else if (screen === 'config') await operation({ type: 'RefreshAccounts' });
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
    activeAgent = ''; agentConnected = false; typing = false;
    agentListeners.splice(0).forEach(unlisten => unlisten());
    if (slug) {
      const closed = await Promise.allSettled([ipc.closeAgent(slug), ipc.closeActivity(slug)]);
      for (const result of closed) if (result.status === 'rejected') showError(errorText(result.reason));
    }
  }
  function selectAgent(slug: string): Promise<void> {
    selectedAgent = slug;
    const generation = ++agentGeneration;
    agentOpening = true;
    agentTransition = agentTransition.catch(error => showError(errorText(error))).then(async () => {
      await stopAgent();
      if (generation !== agentGeneration || disposed || mode !== 'agent') return;
      messages = []; activity = []; schedules = []; pendingAgentUpdates = {}; follow = true;
      agentRevision++;
      activeAgent = slug;
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
        if (generation !== agentGeneration || disposed || mode !== 'agent') { await stopAgent(); return; }
        await Promise.all([ipc.openAgent(slug), ipc.openActivity(slug)]);
        if (generation !== agentGeneration || disposed) { await stopAgent(); return; }
        agentConnected = true;
        report(refreshSchedules());
        focusToken++;
      } catch (error) {
        await stopAgent();
        throw error;
      } finally { if (generation === agentGeneration) agentOpening = false; }
    });
    return agentTransition;
  }
  async function refreshSchedules() {
    if (!selectedAgent) return;
    const slug = selectedAgent;
    const result = await ipc.schedules(slug);
    if (selectedAgent === slug) schedules = result.schedules;
  }
  async function sendAgent(text: string, attachments: InputAttachment[]) {
    if (attachments.length) throw new Error('images go to code sessions for now');
    if (!selectedAgent || !agentConnected) throw new Error('Select a connected agent first.');
    await ipc.sendAgent(selectedAgent, text);
  }
  async function switchMode(next: 'code' | 'agent') {
    mode = next; search = ''; switcher = false; follow = true;
    if (next === 'agent') {
      if (!agents.length) await loadAgents();
      const slug = selectedAgent || agents[0]?.slug;
      if (slug && (!agentConnected || activeAgent !== slug)) await selectAgent(slug);
    } else {
      agentGeneration++;
      agentTransition = agentTransition.catch(error => showError(errorText(error))).then(stopAgent);
      await agentTransition;
      agentOpening = false;
    }
  }
  async function newConversation() {
    if (mode === 'code') {
      if (selectedProject) await openCode(selectedProject, { type: 'New' });
      else await addProject();
    } else {
      agentDraft = ''; agentAttachments = [];
      if (!agentConnected && (selectedAgent || agents[0]?.slug)) await selectAgent(selectedAgent || agents[0].slug);
      focusToken++;
    }
  }
  async function showCommands() {
    if (mode !== 'code') await switchMode('code');
    if (!session) await newConversation();
    if (session) { paletteToken++; focusToken++; }
  }
  function keyboard(event: KeyboardEvent) {
    if (overlay || event.defaultPrevented || event.isComposing) return;
    if (event.metaKey || event.ctrlKey) {
      if (event.key === '1' || event.key === '2') { event.preventDefault(); report(switchMode(event.key === '1' ? 'code' : 'agent')); }
      else if (event.key.toLowerCase() === 'n') { event.preventDefault(); report(newConversation()); }
      else if (event.key.toLowerCase() === 'k') { event.preventDefault(); report(showCommands()); }
    } else if (event.key === 'Escape') {
      if (switcher) switcher = false;
      else if (code.busy && mode === 'code') { event.preventDefault(); report(interrupt()); }
    }
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
    code.revision; agentRevision; typing; mode;
    if (follow) void tick().then(() => { if (transcript) transcript.scrollTop = transcript.scrollHeight; });
    else unseen = true;
  });
  onMount(() => {
    if (overlay) { document.documentElement.classList.add('overlay-surface'); return; }
    report(loadProjects()); report(loadAgents()); report(refreshHealth());
    const health = setInterval(() => report(refreshHealth()), 5000);
    let haltedOff: UnlistenFn | undefined;
    report(listen('computer:halted', () => report(refreshHealth())).then(unlisten => { if (disposed) unlisten(); else haltedOff = unlisten; }));
    return () => {
      disposed = true; sessionGeneration++; agentGeneration++;
      clearInterval(health); clearTimeout(refreshTimer); haltedOff?.();
      sessionListeners.splice(0).forEach(unlisten => unlisten());
      agentListeners.splice(0).forEach(unlisten => unlisten());
      if (session) void ipc.closeSession(session.id).catch(() => {});
      if (activeAgent) { void ipc.closeAgent(activeAgent).catch(() => {}); void ipc.closeActivity(activeAgent).catch(() => {}); }
    };
  });
</script>

<svelte:window onkeydown={keyboard} />

{#if overlay}
  <div class="computer-overlay"><div>goat is driving <span>·</span> ⌘⇧Esc to stop</div></div>
{:else}
  <div class="desktop-shell" class:inspector-hidden={!inspectorOpen}>
    <aside class="sidebar">
      <div class="sidebar-titlebar" data-tauri-drag-region><span class="wordmark" data-tauri-drag-region>goat<span>desktop</span></span></div>
      <div class="mode-switcher"><button class="mode-switcher-button" aria-expanded={switcher} aria-haspopup="menu" onclick={() => switcher = !switcher}><span class="mode-icon"><Icon name={mode === 'code' ? 'code' : 'agent'} size={19} /></span><span><strong>{mode === 'code' ? 'Code' : 'Agent'}</strong><small>{mode === 'code' ? 'Build, debug, and ship' : 'Delegate work to your agent'}</small></span><Icon name="down" size={14} /></button>
        {#if switcher}<div class="mode-menu" role="menu"><button role="menuitem" class:selected={mode === 'code'} onclick={() => report(switchMode('code'))}><Icon name="code" /><span><strong>Code</strong><small>Build, debug, and ship</small></span><kbd>⌘1</kbd></button><button role="menuitem" class:selected={mode === 'agent'} onclick={() => report(switchMode('agent'))}><Icon name="agent" /><span><strong>Agent</strong><small>Delegate work to your agent</small></span><kbd>⌘2</kbd></button></div>{/if}
      </div>
      <div class="sidebar-search search-box"><Icon name="search" size={14} /><input aria-label={mode === 'code' ? 'Search projects and conversations' : 'Search agents'} placeholder={mode === 'code' ? 'Search projects…' : 'Search agents…'} bind:value={search} /><kbd>⌕</kbd></div>
      <button class="new-session" onclick={() => report(newConversation())}><Icon name="plus" size={15} />{mode === 'code' ? 'New session' : 'New message'}<kbd>⌘N</kbd></button>
      <div class="sidebar-section-title"><span>{mode === 'code' ? 'Projects' : 'Your agents'}</span><div>{#if mode === 'code'}<button class="icon-button" title="Add project folder" aria-label="Add project folder" onclick={() => report(addProject())}><Icon name="plus" size={14} /></button>{:else}<button class="icon-button" title="Refresh agents" aria-label="Refresh agents" disabled={agentLoading} onclick={() => report(loadAgents())}><Icon name="refresh" size={13} /></button>{/if}</div></div>
      <div class="sidebar-list">
        {#if mode === 'code'}
          {#each filteredProjects as project (project.path)}
            <div class="project-item"><div class="project-row" class:active={selectedProject === project.path}><button class="project-toggle" title={project.path} aria-expanded={expanded.includes(project.path)} onclick={() => report(toggleProject(project.path))}><span class:expanded={expanded.includes(project.path)} class="expand-arrow"><Icon name="chevron" size={11} /></span><Icon name="folder" size={15} /><span>{project.path.split('/').filter(Boolean).at(-1) || project.path}</span></button><button class="project-remove icon-button" aria-label={`Remove ${project.path} from projects`} title="Remove from projects (files are not deleted)" onclick={() => report(removeProject(project.path))}><Icon name="close" size={12} /></button></div>
            {#if expanded.includes(project.path)}<div class="conversation-list">{#each (projectConversations[project.path] ?? []).filter(conversation => !search || project.path.toLowerCase().includes(search.toLowerCase()) || conversation.title?.toLowerCase().includes(search.toLowerCase())) as conversation (conversation.conversation_id)}<button class:active={session?.cwd === project.path && code.conversation === conversation.conversation_id} title={conversation.title ?? 'Untitled conversation'} onclick={() => report(openCode(project.path, { type: 'Conversation', conversation_id: conversation.conversation_id }))}><span class="conversation-mark" class:live={conversation.live != null}></span><span>{conversation.title || 'Untitled conversation'}</span></button>{/each}<button class="project-new" onclick={() => report(openCode(project.path, { type: 'New' }))}><Icon name="plus" size={12} /> New session</button></div>{/if}</div>
          {/each}
          {#if projectLoading}<p class="sidebar-empty"><span class="spinner"></span> Loading projects…</p>{:else if !projects.length}<div class="sidebar-empty"><p>A home for every project.</p><button class="text-button" onclick={() => report(addProject())}>Add your first folder <Icon name="arrow" size={12} /></button></div>{:else if !filteredProjects.length}<p class="sidebar-empty">No matching projects.</p>{/if}
        {:else}
          {#each filteredAgents as agent (agent.slug)}<button class="agent-row" class:active={selectedAgent === agent.slug} onclick={() => report(selectAgent(agent.slug))}><span class="agent-avatar">{(agent.display || agent.slug).slice(0, 1).toUpperCase()}</span><span><strong>{agent.display || agent.slug}</strong><small>{agent.slug}</small></span>{#if selectedAgent === agent.slug && agentConnected}<span class="status-dot"></span>{/if}</button>{/each}
          {#if agentLoading}<p class="sidebar-empty"><span class="spinner"></span> Loading agents…</p>{:else if !agents.length}<div class="sidebar-empty"><p>No agents configured.</p><p>Create an agent with the goat CLI, then refresh this list.</p></div>{:else if !filteredAgents.length}<p class="sidebar-empty">No matching agents.</p>{/if}
        {/if}
      </div>
      <div class="sidebar-bottom"><span class="local-badge"><span class="status-dot" class:offline={!daemon?.ready}></span> Local workspace</span><button class="icon-button" title="Command palette · ⌘K" aria-label="Open command palette" onclick={() => report(showCommands())}><Icon name="terminal" size={15} /></button></div>
    </aside>
    <main class="main-column">
      <header class="main-header" data-tauri-drag-region>
        <div class="header-title" data-tauri-drag-region>{#if mode === 'code'}<div class="header-breadcrumb" data-tauri-drag-region>{#if workspace?.branch}<Icon name="git" size={13} />{workspace.repo?.split('/').at(-1)}<span>/</span><strong>{workspace.branch}</strong>{:else}<span>{session ? session.cwd.split('/').filter(Boolean).at(-1) : 'Code workspace'}</span>{/if}{#if workspace?.pr}<span class="pr-badge" class:merged={workspace.pr.state.toLowerCase() === 'merged'}>PR #{workspace.pr.number} · {workspace.pr.state.toLowerCase()}</span>{/if}</div><h1 data-tauri-drag-region>{title}</h1>{:else}<div class="header-breadcrumb" data-tauri-drag-region><span class="status-dot" class:offline={!agentConnected}></span>{agentConnected ? 'Connected' : agentOpening ? 'Connecting…' : 'Agent workspace'}</div><h1 data-tauri-drag-region>{currentAgent?.display || currentAgent?.slug || 'A little more possible'}</h1>{/if}</div>
        <div class="header-actions">{#if mode === 'code' && session}{#if connection === 'closed'}<button class="secondary" onclick={() => report(openCode(session!.cwd, code.conversation ? { type: 'Conversation', conversation_id: code.conversation } : { type: 'Latest' }))}><Icon name="refresh" size={13} />Reconnect</button>{:else}<button class="model-button" onclick={() => report(openPanel('model'))}><Icon name="cpu" size={13} /><span>{code.model?.model ?? 'Choose model'}</span><Icon name="down" size={11} /></button>{#if code.contextWindow}<button class="context-meter" title={`${compact.format(code.contextTokens ?? 0)} of ${compact.format(code.contextWindow)} context tokens`} aria-label="View context usage" onclick={() => report(openPanel('usage'))}><svg viewBox="0 0 28 28" aria-hidden="true"><circle cx="14" cy="14" r="10" /><circle class="meter-value" cx="14" cy="14" r="10" pathLength="100" stroke-dasharray={`${Math.min(100, (code.contextTokens ?? 0) / code.contextWindow * 100)} 100`} /></svg></button>{/if}{/if}{/if}<button class="icon-button" class:active={inspectorOpen} title="Toggle inspector" aria-label="Toggle inspector" aria-pressed={inspectorOpen} onclick={() => inspectorOpen = !inspectorOpen}><Icon name="panel" size={17} /></button></div>
      </header>
      {#if errors.length || daemonError || computerError}<div class="app-errors" role="alert">{#if daemonError}<div><span><strong>Daemon unavailable</strong>{daemonError}</span><button class="icon-button" aria-label="Retry daemon connection" onclick={() => report(refreshHealth())}><Icon name="refresh" size={14} /></button></div>{/if}{#if computerError}<div><span><strong>Computer provider unavailable</strong>{computerError}</span><button class="icon-button" aria-label="Retry computer status" onclick={() => report(refreshHealth())}><Icon name="refresh" size={14} /></button></div>{/if}{#each errors as error (error.id)}<div><span>{error.message}</span><button class="icon-button" aria-label="Dismiss error" onclick={() => errors = errors.filter(item => item.id !== error.id)}><Icon name="close" size={14} /></button></div>{/each}</div>{/if}
      <div class="transcript-scroll" bind:this={transcript} onscroll={scroll}>
        {#if mode === 'code'}
          {#if connection === 'opening'}<div class="welcome"><span class="spinner large-spinner"></span><h2>Opening your workspace</h2><p>Connecting to the local daemon…</p></div>{:else if session}<Transcript state={code} onop={operation} onerror={showError} />{#if !code.rows.length && !code.busy}<div class="session-welcome"><span class="welcome-glyph">g</span><h2>What are we building?</h2><p>Explore an idea. Untangle a bug. Make something work.</p><div class="welcome-hints"><span><Icon name="code" size={14} /> Code with context</span><span><Icon name="shield" size={14} /> On your machine</span></div></div>{/if}{:else}<div class="welcome"><span class="welcome-glyph">g</span><div class="eyebrow">Your ideas, in motion</div><h2>A clear space<br />for your next good idea.</h2><p>Open a project and work alongside goat.<br />From the first question to the final commit.</p><button class="primary" onclick={() => report(selectedProject ? openCode(selectedProject, { type: 'New' }) : addProject())}><Icon name="plus" size={16} />{selectedProject ? 'Start a session' : 'Open a project'}</button><span class="welcome-shortcut">or press ⌘N</span></div>{/if}
        {:else if currentAgent}
          <div class="transcript-content agent-transcript">{#each messages as message (message.id)}<article class="transcript-message" class:user-message={!message.outgoing} class:assistant-message={message.outgoing}><div class="message-label"><span class="avatar" class:goat-avatar={message.outgoing} class:user-avatar={!message.outgoing}>{message.outgoing ? (currentAgent.display || currentAgent.slug).slice(0, 1).toUpperCase() : 'Y'}</span>{message.outgoing ? currentAgent.display || currentAgent.slug : 'You'}<time datetime={message.ts}>{new Date(message.ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time></div>{#if message.outgoing}<Markdown text={message.text} onerror={showError} />{:else}<div class="user-text">{message.text}</div>{/if}</article>{/each}{#if typing}<div class="working-row"><span class="typing-indicator"><i></i><i></i><i></i></span>{currentAgent.display || currentAgent.slug} is thinking</div>{/if}</div>
          {#if !messages.length && !typing}<div class="session-welcome"><span class="agent-avatar large-avatar">{(currentAgent.display || currentAgent.slug).slice(0, 1).toUpperCase()}</span><h2>{agentOpening ? 'Opening your conversation…' : `Meet ${currentAgent.display || currentAgent.slug}`}</h2><p>{agentOpening ? 'Loading messages from your daemon.' : 'Delegate a task, ask a question, or plan what comes next.'}</p></div>{/if}
        {:else}<div class="welcome"><span class="welcome-glyph"><Icon name="agent" size={36} /></span><div class="eyebrow">A capable companion</div><h2>Good work<br />doesn’t have to wait.</h2><p>Select an agent to start a conversation.<br />Its routines, connections, and activity live here.</p>{#if !agents.length}<button class="secondary" onclick={() => report(loadAgents())}><Icon name="refresh" size={14} />Refresh agents</button>{/if}</div>{/if}
      </div>
      {#if unseen}<button class="jump-latest" onclick={() => report(latest())}>Jump to latest<Icon name="down" size={13} /></button>{/if}
      {#if mode === 'code' && code.notices.length}<div class="toast-stack" aria-live="polite">{#each code.notices as notice (notice.key)}<div class:error-toast={notice.level === 'error'} class="toast"><Icon name={notice.level === 'error' ? 'close' : notice.level === 'success' ? 'check' : 'agent'} size={14} /><span>{notice.text}</span><button class="icon-button" aria-label="Dismiss notification" onclick={() => code.notices = code.notices.filter(item => item.key !== notice.key)}><Icon name="close" size={12} /></button></div>{/each}</div>{/if}
      {#if mode === 'code' && code.queued.length}<div class="queued-messages">{#each code.queued as queued}<div><span class="pulse-dot"></span><span>Queued</span><p>{queued.text || `${queued.attachments.length} image attachments`}</p></div>{/each}</div>{/if}
      {#if mode === 'code'}{#key session?.id}<Composer bind:value={codeDraft} bind:attachments={codeAttachments} {mode} busy={code.busy} disabled={connection !== 'connected'} plan={code.mode === 'plan'} {specs} files={code.files} {focusToken} {paletteToken} onsubmit={sendCode} oninterrupt={interrupt} onop={operation} onerror={showError} />{/key}{:else}{#key selectedAgent}<Composer bind:value={agentDraft} bind:attachments={agentAttachments} {mode} busy={typing} disabled={!agentConnected} {focusToken} onsubmit={sendAgent} oninterrupt={interrupt} onop={operation} onerror={showError} />{/key}{/if}
    </main>
    {#if inspectorOpen}<aside class="inspector">{#if mode === 'code' && session}<SessionPanel screen={panel} state={code} {session} {workspace} {daemon} {presence} {specs} onop={operation} onadmin={admin} onpanel={screen => report(openPanel(screen))} onclose={() => panel = 'overview'} onerror={showError} />{:else if mode === 'agent' && currentAgent}<AgentPanel agent={currentAgent} {schedules} {activity} onrefresh={refreshSchedules} onerror={showError} />{:else}<div class="inspector-heading"><span>{mode === 'code' ? 'Your workspace' : 'Agent details'}</span><Icon name="panel" size={15} /></div><div class="inspector-body"><div class="inspector-intro"><span class="intro-line"></span><h3>Everything in context.</h3><p>{mode === 'code' ? 'Models, usage, and workspace details stay close to the work.' : 'Routines, connections, and live activity, all in one place.'}</p></div><div class="local-info"><Icon name="shield" size={19} /><h3>Local by design</h3><p>Your desktop connects directly to the daemon on this machine.</p></div></div>{/if}</aside>{/if}
    <footer class="statusbar"><div class="daemon-status" title={daemonError || (daemon ? `goat ${daemon.version} · PID ${daemon.pid} · ${daemon.sessions} sessions` : 'Connecting to daemon')}><span class="status-dot" class:offline={!daemon?.ready}></span>{daemon?.ready ? 'Daemon connected' : daemonError ? 'Daemon unavailable' : 'Connecting…'}{#if daemon}<span class="status-version">v{daemon.version}</span>{/if}</div><div class="computer-status">{#if computer}<Icon name="shield" size={12} />{#if computer.halted}<span class="stopped-label">Stopped</span><button disabled={permissionPending} onclick={() => report(requestPermission('resume'))}>Resume computer use</button>{:else if !computer.accessibility || !computer.screen}<span>Computer use</span>{#if !computer.accessibility}<button disabled={permissionPending} onclick={() => report(requestPermission('accessibility'))}>Grant Accessibility</button>{/if}{#if !computer.screen}<button disabled={permissionPending} onclick={() => report(requestPermission('screen'))}>Grant Screen Recording</button>{/if}{:else}<span class:driving={computer.busy}>{computer.busy ? 'goat is driving' : computer.advertised ? 'Computer ready' : 'Connecting computer…'}</span><kbd>⌘⇧Esc to stop</kbd>{/if}{:else}<span>Computer {computerError ? 'unavailable' : 'connecting…'}</span>{/if}</div></footer>
  </div>
{/if}

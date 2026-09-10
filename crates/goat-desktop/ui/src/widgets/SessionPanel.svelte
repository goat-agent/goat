<script lang="ts">
  import type { DaemonStatus2, Op, RewindScope } from '../shared/api';
  import type { AdminRequest, CommandSpec, DesktopSessionInfo, Workspace } from '../shared/ipc';
  import type { SessionView } from '../entities/session';
  import Icon from '../shared/Icon.svelte';
  import Markdown from '../shared/Markdown.svelte';
  let { screen, state: view, session, workspace, daemon, presence, specs, onop, onadmin, onpanel, onclose, onerror }: {
    screen: string; state: SessionView; session: DesktopSessionInfo; workspace: Workspace | null;
    daemon: DaemonStatus2 | null; presence: number; specs: CommandSpec[];
    onop: (op: Op) => Promise<void>; onadmin: (request: AdminRequest) => Promise<void>;
    onpanel: (screen: string) => void; onclose: () => void; onerror: (message: string) => void;
  } = $props();
  let query = $state('');
  let accountNames = $state<Record<string, string>>({});
  let secrets = $state<Record<string, string>>({});
  let pending = $state('');
  let rewindScope = $state<RewindScope>('code_and_conversation');
  const number = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });
  const titles: Record<string, string> = { overview: 'Session', model: 'Choose a model', effort: 'Reasoning effort', config: 'Providers & accounts', status: 'Session status', usage: 'Usage & limits', help: 'Commands', resume: 'Resume conversation', rewind: 'Rewind' };
  const currentModel = $derived(view.models.find(model => model.provider === view.model?.provider && model.model === view.model?.model));
  $effect(() => { screen; query = ''; });
  async function dispatch(op: Op, close = false) {
    pending = JSON.stringify(op);
    try { await onop(op); if (close) onclose(); }
    catch (error) { onerror(String(error)); }
    finally { pending = ''; }
  }
  async function admin(request: AdminRequest, key: string) {
    pending = key;
    try { await onadmin(request); if (request.type === 'ProviderLogin') secrets[request.provider] = ''; await onop({ type: 'RefreshAccounts' }); }
    catch (error) { onerror(String(error)); }
    finally { pending = ''; }
  }
</script>

<div class="inspector-heading"><span>{titles[screen] ?? screen}</span>{#if screen !== 'overview'}<button class="icon-button" title="Back to session" aria-label="Close panel" onclick={onclose}><Icon name="close" size={15} /></button>{:else}<Icon name="panel" size={15} />{/if}</div>
<div class="inspector-body">
  {#if screen === 'model'}
    <div class="search-box"><Icon name="search" size={14} /><input aria-label="Search models" placeholder="Search models…" bind:value={query} /></div>
    {#if view.model}<button class="selection-row" onclick={() => onpanel('effort')}><span>Reasoning effort</span><span class="capitalize">{view.model.effort ?? 'Default'}<Icon name="chevron" size={12} /></span></button>{/if}
    {#each view.models.filter(model => `${model.provider} ${model.model}`.toLowerCase().includes(query.toLowerCase())) as model}
      <section class="model-entry">
        <div class="model-name"><strong>{model.model}</strong><small>{model.provider} · {model.context_window ? `${number.format(model.context_window)} context` : 'Context not reported'}{model.supports_images ? ' · vision' : ''}</small></div>
        {#each model.accounts as account}
          <button class="selection-row" class:selected={view.model?.model === model.model && view.model?.provider === model.provider && view.model?.account === account.id} disabled={!!pending} onclick={() => dispatch({ type: 'SelectModel', target: account.target }, true)}><span>{account.display || account.id}</span>{#if view.model?.model === model.model && view.model?.provider === model.provider && view.model?.account === account.id}<Icon name="check" size={14} />{:else}<Icon name="arrow" size={14} />{/if}</button>
        {/each}
        {#if !model.accounts.length}<p class="muted small">No connected account. <button class="text-button" onclick={() => onpanel('config')}>Connect one</button></p>{/if}
      </section>
    {/each}
    {#if !view.models.length}<div class="panel-empty"><Icon name="cpu" size={25} /><h3>No models available</h3><p>Connect a provider to choose a model.</p><button class="secondary" onclick={() => onpanel('config')}>Open providers</button></div>{/if}
  {:else if screen === 'effort'}
    <p class="muted">How much reasoning should {view.model?.model ?? 'the model'} use?</p>
    {#each currentModel?.efforts ?? [] as effort}<button class="selection-row" class:selected={view.model?.effort === effort} disabled={!!pending || !view.model} onclick={() => view.model && dispatch({ type: 'SelectModel', target: { ...view.model, effort } }, true)}><span class="capitalize">{effort}</span>{#if view.model?.effort === effort}<Icon name="check" size={14} />{/if}</button>{/each}
    {#if !currentModel?.efforts?.length}<p class="panel-note">{view.model ? 'This model does not support adjustable reasoning effort.' : 'Choose a model first.'}</p>{/if}
  {:else if screen === 'config'}
    <p class="muted small">Credentials stay with your local daemon. OAuth opens your browser.</p>
    {#each view.accounts as provider}
      <section class="provider-card">
        <h3>{provider.display_name || provider.provider}<span class="chip">{provider.local ? 'local' : provider.provider}</span></h3>
        {#each provider.accounts as account}
          <div class="account-row"><span>{account.name}<small>{account.method.replaceAll('_', ' ')}</small></span><button class="quiet danger" disabled={!!pending} onclick={() => admin({ type: 'CredentialRemove', key: { provider: provider.provider, account: account.name, service: 'model', slot: null } }, `remove:${provider.provider}:${account.name}`)}>Remove</button></div>
        {/each}
        {#if !provider.accounts.length}<p class="muted small">No connected accounts</p>{/if}
        {#if provider.login !== 'none'}
          <label class="field-label" for={`account-${provider.provider}`}>Account name</label><input id={`account-${provider.provider}`} class="text-field" placeholder="default" value={accountNames[provider.provider] ?? ''} oninput={event => accountNames[provider.provider] = event.currentTarget.value} />
          {#if provider.login === 'api_key' || provider.login === 'api_key_or_o_auth'}
            <label class="field-label" for={`secret-${provider.provider}`}>API key</label><input id={`secret-${provider.provider}`} class="text-field" type="password" autocomplete="off" value={secrets[provider.provider] ?? ''} oninput={event => secrets[provider.provider] = event.currentTarget.value} />
            <button class="secondary full-width" disabled={!!pending || !secrets[provider.provider]?.trim()} onclick={() => admin({ type: 'ProviderLogin', provider: provider.provider, account: accountNames[provider.provider]?.trim() || 'default', method: { type: 'ApiKey', secret: secrets[provider.provider] } }, provider.provider)}>Connect API key</button>
          {/if}
          {#if provider.login === 'o_auth' || provider.login === 'api_key_or_o_auth'}<button class="secondary full-width" disabled={!!pending} onclick={() => admin({ type: 'ProviderLogin', provider: provider.provider, account: accountNames[provider.provider]?.trim() || 'default', method: { type: 'OAuth' } }, provider.provider)}>Log in with browser<Icon name="external" size={13} /></button>{/if}
        {/if}
        {#each view.loginStatus.filter(status => status.provider === provider.provider) as status}<div class:login-error={status.done && !status.ok} class="login-status"><Markdown text={status.message || (status.done && status.ok ? 'Connected successfully' : 'Waiting for browser login…')} {onerror} /></div>{/each}
      </section>
    {/each}
    {#if !view.accounts.length}<p class="panel-note">No provider accounts have been reported by the daemon.</p>{/if}
    <button class="quiet full-width" disabled={!!pending} onclick={() => dispatch({ type: 'RefreshAccounts' })}><Icon name="refresh" size={14} /> Refresh accounts</button>
  {:else if screen === 'help'}
    <div class="search-box"><Icon name="search" size={14} /><input aria-label="Search commands" placeholder="Find a command…" bind:value={query} /></div>
    {#each specs.filter(spec => `${spec.name} ${spec.description} ${spec.aliases?.join(' ')}`.toLowerCase().includes(query.toLowerCase())) as spec}<section class="command-reference"><strong>/{spec.name}</strong><p>{spec.description}</p><code>{spec.usage}</code>{#if spec.aliases?.length}<small>Aliases: {spec.aliases.map(alias => `/${alias}`).join(', ')}</small>{/if}</section>{/each}
    <section class="shortcut-reference"><h3>Keyboard shortcuts</h3><dl><dt>Code / Agent</dt><dd>⌘1 / ⌘2</dd><dt>New session / chat</dt><dd>⌘N</dd><dt>Commands</dt><dd>⌘K</dd><dt>Interrupt turn</dt><dd>Esc</dd><dt>Stop computer use</dt><dd>⌘⇧Esc</dd></dl></section>
  {:else if screen === 'resume'}
    <button class="quiet full-width" disabled={!!pending} onclick={() => dispatch({ type: 'ListConversations' })}><Icon name="refresh" size={14} /> Refresh conversations</button>
    {#each view.conversations as conversation}<button class="conversation-choice" class:selected={view.conversation === conversation.id} disabled={!!pending} onclick={() => dispatch({ type: 'Resume', conversation_id: conversation.id }, true)}><strong>{conversation.title || 'Untitled conversation'}</strong><small>{conversation.model} · {new Date(conversation.updated_at * 1000).toLocaleDateString()}{conversation.live ? ' · live' : ''}</small></button>{/each}
    {#if !view.conversations.length}<p class="panel-note">No saved conversations in this project.</p>{/if}
  {:else if screen === 'rewind'}
    <p class="muted small">Return to a checkpoint. The selected prompt will be restored to your composer.</p>
    <label class="field-label" for="rewind-scope">Restore</label><select id="rewind-scope" class="text-field" bind:value={rewindScope}><option value="code_and_conversation">Code and conversation</option><option value="conversation">Conversation only</option><option value="code">Code only</option></select>
    <button class="quiet full-width" disabled={!!pending} onclick={() => dispatch({ type: 'ListRewindPoints' })}><Icon name="refresh" size={14} /> Refresh checkpoints</button>
    {#each view.rewind as point}<button class="conversation-choice" disabled={!!pending || view.busy} onclick={() => dispatch({ type: 'Rewind', checkpoint_id: point.checkpoint_id, scope: rewindScope }, true)}><strong>{point.prompt}</strong><small>{new Date(point.created_at * 1000).toLocaleString()} · {point.code_changes ? 'Code changes' : 'Conversation'}</small></button>{/each}
    {#if view.busy}<p class="panel-note">Wait for the active turn to finish before rewinding.</p>{/if}
    {#if !view.rewind.length}<p class="panel-note">This conversation has no rewind checkpoints.</p>{/if}
  {:else if screen === 'usage'}
    <div class="metric-hero"><span>{number.format(view.turnTokens)}</span><small>tokens this turn</small></div>
    {#each view.usage as entry}<section class="usage-card"><h3>{entry.provider}<small>{entry.account}</small></h3><dl><dt>Input</dt><dd>{number.format(entry.usage.input_tokens)}</dd><dt>Output</dt><dd>{number.format(entry.usage.output_tokens)}</dd><dt>Cache read</dt><dd>{number.format(entry.usage.cache_read_tokens)}</dd><dt>Cache write</dt><dd>{number.format(entry.usage.cache_write_tokens)}</dd><dt>Session input</dt><dd>{number.format(view.totals[`${entry.provider}/${entry.account}`]?.input ?? 0)}</dd><dt>Session output</dt><dd>{number.format(view.totals[`${entry.provider}/${entry.account}`]?.output ?? 0)}</dd>{#if entry.context_window}<dt>Context window</dt><dd>{number.format(entry.context_window)}</dd>{/if}</dl></section>{/each}
    {#if !view.usage.length}<p class="panel-note">Usage appears after your first model response.</p>{/if}
    {#each view.rates as rate}<section class="usage-card"><h3>{rate.provider} limits<small>{rate.account}</small></h3>{#each rate.snapshot.windows as window}<div class="rate-window"><span>{window.label}<strong>{Math.round(window.used_percent)}%</strong></span><progress max="100" value={window.used_percent}></progress>{#if window.resets_at}<small>Resets {new Date(window.resets_at * 1000).toLocaleString()}</small>{/if}</div>{/each}<small class="muted">Updated {new Date(rate.cached_at * 1000).toLocaleTimeString()}</small></section>{/each}
  {:else if screen === 'status' || screen === 'overview'}
    <div class="session-insight"><span class="eyebrow">Working directory</span><h3 class="path">{session.cwd.split('/').filter(Boolean).at(-1) || session.cwd}</h3><p class="muted small path">{session.cwd}</p>{#if workspace?.branch}<span class="branch-chip"><Icon name="git" size={12} />{workspace.branch}</span>{/if}</div>
    <dl class="session-details"><dt>State</dt><dd>{view.asks.length ? 'Waiting on you' : view.busy ? 'Working' : 'Ready'}</dd><dt>Mode</dt><dd class="capitalize">{view.mode}</dd><dt>Model</dt><dd>{view.model?.model ?? 'Not selected'}</dd><dt>Provider</dt><dd>{view.model?.provider ?? '—'}</dd><dt>Account</dt><dd>{view.model?.account ?? '—'}</dd><dt>Effort</dt><dd>{view.model?.effort ?? 'Default'}</dd><dt>Connected windows</dt><dd>{presence}</dd><dt>Queued messages</dt><dd>{view.queued.length}</dd>{#if screen === 'status'}<dt>Desktop session</dt><dd>{session.id}</dd><dt>Conversation</dt><dd>{view.conversation ?? 'Not bound yet'}</dd><dt>Daemon</dt><dd>{daemon?.version ?? session.daemon.version}</dd><dt>Daemon PID</dt><dd>{daemon?.pid ?? '—'}</dd><dt>Workspace kind</dt><dd>{workspace?.kind ?? '—'}</dd>{/if}</dl>
    {#if view.contextWindow}<section class="context-card"><div><span>Context</span><span>{Math.round((view.contextTokens ?? 0) / view.contextWindow * 100)}%</span></div><progress value={view.contextTokens ?? 0} max={view.contextWindow}></progress><small>{number.format(view.contextTokens ?? 0)} / {number.format(view.contextWindow)} tokens{view.compactionThreshold ? ` · compacts at ${number.format(view.compactionThreshold)}` : ''}</small></section>{/if}
    <div class="inspector-actions"><button class="selection-row" onclick={() => onpanel('model')}><span><Icon name="cpu" /> Model & reasoning</span><Icon name="chevron" size={13} /></button><button class="selection-row" onclick={() => onpanel('config')}><span><Icon name="settings" /> Providers & accounts</span><Icon name="chevron" size={13} /></button><button class="selection-row" onclick={() => onpanel('usage')}><span><Icon name="clock" /> Usage & limits</span><Icon name="chevron" size={13} /></button><button class="selection-row" onclick={() => onpanel('help')}><span><Icon name="terminal" /> All commands</span><Icon name="chevron" size={13} /></button></div>
    {#if view.processes.length}<section class="usage-card"><h3>Processes</h3>{#each view.processes as process}<div class="process-inspector-row"><span class="status-dot" class:running={process.state === 'running'}></span><span class="path">{process.command}</span>{#if process.state === 'running'}<button class="icon-button danger" aria-label={`Stop ${process.command}`} onclick={() => dispatch({ type: 'ProcessKill', process: process.id })}><Icon name="stop" size={12} /></button>{/if}</div>{/each}</section>{/if}
    {#if view.planPath}<p class="muted small path">Plan: {view.planPath}</p>{/if}
  {:else}
    <div class="error-banner" role="alert">The command returned an unsupported panel: {screen}</div>
  {/if}
</div>

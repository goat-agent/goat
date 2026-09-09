<script lang="ts">
  import type { Op } from '../lib/api';
  import type { Row, SessionView } from '../lib/session';
  import { imageSource } from '../lib/markdown';
  import Icon from './Icon.svelte';
  import Markdown from './Markdown.svelte';
  import AskCard from './AskCard.svelte';
  import PlanCard from './PlanCard.svelte';
  let { state, onop, onerror }: { state: SessionView; onop: (op: Op) => Promise<void>; onerror: (message: string) => void } = $props();
  const number = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });
  const ansi = /\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))/g;
</script>

{#snippet rows(items: Row[])}
  {#each items as row (row.key)}
    {#if row.kind === 'user'}
      <article class="transcript-message user-message">
        <div class="message-label"><span class="avatar user-avatar">Y</span> You</div>
        <div class="user-text">{row.text}</div>
        {#if row.attachments?.length}
          <div class="attachment-grid">{#each row.attachments as attachment}<figure>{#if imageSource(attachment)}<img src={imageSource(attachment)} alt={attachment.label} loading="lazy" />{:else}<span>Unsupported image type: {attachment.media_type}</span>{/if}<figcaption>{attachment.label}</figcaption></figure>{/each}</div>
        {/if}
      </article>
    {:else if row.kind === 'text'}
      <article class:streaming={row.streaming} class="transcript-message assistant-message">
        <div class="message-label"><span class="avatar goat-avatar">g</span> goat {#if row.streaming}<span class="live-label">Writing</span>{/if}</div>
        <Markdown text={row.text} {onerror} />
      </article>
    {:else if row.kind === 'thinking'}
      <details class="thinking-row">
        <summary><Icon name="cpu" /><span>{row.streaming ? 'Thinking…' : 'Reasoning'}</span><Icon name="down" size={12} /></summary>
        <Markdown text={row.text} {onerror} />
      </details>
    {:else if row.kind === 'system'}
      <div class="system-row"><Markdown text={row.text} {onerror} /></div>
    {:else if row.kind === 'tool'}
      <details class="tool-card" class:failed={row.outcome && !row.outcome.ok}>
        <summary>
          <span class="tool-icon">{#if !row.outcome}<span class="spinner"></span>{:else}<Icon name={row.outcome.ok ? 'check' : 'close'} size={14} />{/if}</span>
          <strong>{row.call.name}</strong><span class="tool-primary">{row.call.display.primary}</span><Icon name="down" size={12} />
        </summary>
        {#if row.call.display.detail}<pre class="tool-detail">{row.call.display.detail}</pre>{/if}
        {#if row.outcome?.summary}<Markdown text={row.outcome.summary} {onerror} />{/if}
        {#if row.outcome?.body}<pre class="tool-output">{row.outcome.body.replace(ansi, '')}</pre>{/if}
        {#if !row.outcome}<p class="muted small">Running…</p>{/if}
      </details>
      {#if row.outcome?.image && imageSource(row.outcome.image)}
        <details class="image-result" open><summary>Screenshot <Icon name="down" size={12} /></summary><img src={imageSource(row.outcome.image)} alt={`${row.call.name} screenshot`} loading="lazy" /></details>
      {/if}
    {:else if row.kind === 'group'}
      <section class="subagent-group">
        <div class="eyebrow"><Icon name="agent" /> {row.members.length} {row.members.length === 1 ? 'agent' : 'agents'}</div>
        {#each row.members as member (member.member.call)}
          {@const run = member.run ? state.subagents[member.run] : undefined}
          <details class="subagent-card">
            <summary><span class="status-dot" class:running={member.done === undefined} class:failed={member.done === false}></span><span><strong>{member.member.label || member.member.subagent_type}</strong><small>{member.member.subagent_type}{member.member.background ? ' · background' : ''}</small></span>{#if run}<span class="muted small">{run.tools} tools · {number.format(run.tokens)} tokens</span>{/if}<Icon name="down" size={12} /></summary>
            {#if run?.rows.length}<div class="nested-transcript">{@render rows(run.rows)}</div>{:else if member.outcome?.body || member.outcome?.summary}<Markdown text={member.outcome.body ?? member.outcome.summary ?? ''} {onerror} />{:else}<p class="muted small">{member.done === undefined ? 'Working…' : member.done ? 'Completed' : 'Stopped with an error'}</p>{/if}
          </details>
        {/each}
      </section>
    {:else if row.kind === 'shell'}
      <details class="terminal-card" open>
        <summary><Icon name="terminal" /><strong>{row.command || 'Shell'}</strong>{#if !row.done}<span class="spinner"></span>{/if}<Icon name="down" size={12} /></summary>
        <pre>{row.output.replace(ansi, '') || 'Running…'}</pre>
      </details>
    {:else if row.kind === 'process'}
      <details class="terminal-card" open={row.state === 'running'}>
        <summary><Icon name="terminal" /><strong>{row.command || `Process ${row.process ?? ''}`}</strong><span class="muted small">{row.state}{row.exitCode != null ? ` · exit ${row.exitCode}` : ''}{row.reason && row.reason !== 'natural' ? ` · ${row.reason}` : ''}</span><Icon name="down" size={12} /></summary>
        <pre>{row.output.replace(ansi, '') || (row.state === 'running' ? 'Waiting for output…' : 'No output')}</pre>
        {#if row.process && row.state === 'running'}
          {@const process = state.processes.find(process => process.id === row.process)}
          <div class="button-row"><button class="quiet" onclick={() => onop({ type: 'ProcessWatch', process: row.process!, on: !process?.watched }).catch(error => onerror(String(error)))}>{process?.watched ? 'Unwatch' : 'Watch output'}</button><button class="quiet danger" onclick={() => onop({ type: 'ProcessKill', process: row.process! }).catch(error => onerror(String(error)))}>Stop process</button></div>
        {/if}
      </details>
    {:else if row.kind === 'compaction'}
      <div class="system-row compaction-row"><Icon name="cpu" /><span>Context compacted</span><span>{number.format(row.before)} → {number.format(row.after)} tokens</span></div>
    {:else if row.kind === 'notice'}
      <div class:error-banner={row.level === 'error'} class="transcript-notice" role={row.level === 'error' ? 'alert' : 'status'}><strong>{row.text}</strong>{#if row.hint}<p>{row.hint}</p>{/if}</div>
    {/if}
  {/each}
{/snippet}

<div class="transcript-content">
  {@render rows(state.rows)}
  {#each state.asks as ask (ask.call)}<AskCard {ask} {onop} />{/each}
  {#if state.plan}<PlanCard plan={state.plan} {onop} {onerror} />{/if}
  {#if state.compacting}<div class="working-row"><span class="spinner"></span> Compacting context…</div>{/if}
  {#if state.retry}<div class="retry-banner" role="status"><Icon name="refresh" /><span>Retry {state.retry.attempt}/{state.retry.max_attempts} · {state.retry.reason}<small>Waiting {Math.ceil(state.retry.delay_ms / 1000)} seconds before trying again</small></span></div>{/if}
  {#if state.busy && !state.compacting && !state.retry && !state.asks.length && !state.plan}<div class="working-row"><span class="pulse-dot"></span>{state.thinking ? 'Thinking' : 'Working'}<span class="muted">Esc to interrupt</span></div>{/if}
</div>

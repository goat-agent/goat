<script lang="ts">
  import type { AgentActivity, AgentEntry, ScheduleEntry } from '../shared/api';
  import Icon from '../shared/Icon.svelte';
  import Markdown from '../shared/Markdown.svelte';
  let { agent, schedules, activity, onrefresh, onerror }: {
    agent: AgentEntry; schedules: ScheduleEntry[]; activity: AgentActivity[];
    onrefresh: () => Promise<void>; onerror: (message: string) => void;
  } = $props();
  let tab = $state<'routines' | 'connectors' | 'activity'>('routines');
  let refreshing = $state(false);
  async function refresh() {
    refreshing = true;
    try { await onrefresh(); }
    catch (error) { onerror(String(error)); }
    finally { refreshing = false; }
  }
</script>

<div class="inspector-heading"><span>Agent details</span><span class="avatar tiny-avatar">{(agent.display || agent.slug).slice(0, 1).toUpperCase()}</span></div>
<div class="inspector-tabs" role="tablist" aria-label="Agent details">
  <button role="tab" aria-selected={tab === 'routines'} class:active={tab === 'routines'} onclick={() => tab = 'routines'}>Routines</button>
  <button role="tab" aria-selected={tab === 'connectors'} class:active={tab === 'connectors'} onclick={() => tab = 'connectors'}>Connectors</button>
  <button role="tab" aria-selected={tab === 'activity'} class:active={tab === 'activity'} onclick={() => tab = 'activity'}>Activity</button>
</div>
<div class="inspector-body" role="tabpanel" aria-label={tab}>
  {#if tab === 'routines'}
    <div class="section-label">Scheduled work<button class="icon-button" aria-label="Refresh routines" disabled={refreshing} onclick={refresh}><Icon name="refresh" size={13} /></button></div>
    {#each schedules as schedule (schedule.id)}
      <section class="routine-card"><div class="eyebrow"><Icon name="clock" size={13} />{schedule.kind === 'cron' ? 'Recurring' : 'One time'}<span>#{schedule.id}</span></div><Markdown text={schedule.instruction} {onerror} /><dl><dt>{schedule.kind === 'cron' ? 'Cron' : 'When'}</dt><dd>{schedule.kind === 'cron' ? schedule.when : new Date(schedule.when).toLocaleString()}</dd><dt>Next run</dt><dd>{schedule.next_run ? new Date(schedule.next_run).toLocaleString() : 'Not scheduled'}</dd></dl></section>
    {/each}
    {#if !schedules.length}<div class="panel-empty"><Icon name="clock" size={28} /><h3>No active routines</h3><p>Ask your agent to schedule something. Its upcoming work will appear here.</p></div>{/if}
  {:else if tab === 'connectors'}
    <p class="muted small">Connections configured for {agent.display || agent.slug}. Manage them with the goat CLI.</p>
    <section class="connector-section"><h3>Integrations <span>{agent.integrations.length}</span></h3><div class="chip-list">{#each agent.integrations as integration}<span class="connector-chip"><span class="status-dot"></span>{integration}</span>{/each}</div>{#if !agent.integrations.length}<p class="muted small">No integrations configured.</p>{/if}</section>
    <section class="connector-section"><h3>Channels <span>{agent.channels.length}</span></h3><div class="chip-list">{#each agent.channels as channel}<span class="connector-chip"><Icon name="agent" size={13} />{channel}</span>{/each}</div>{#if !agent.channels.length}<p class="muted small">No channels reported.</p>{/if}</section>
  {:else}
    <div class="section-label">Live activity<span class="muted">{activity.length} / 200</span></div>
    <div class="activity-feed">
      {#each activity as item, index (`${item.cursor}-${index}`)}
        <article class="activity-item"><span class="activity-icon"><Icon name={item.t === 'tool_started' ? 'terminal' : item.t === 'schedule_fired' ? 'clock' : item.t === 'turn_finished' ? 'check' : 'agent'} size={14} /></span><div>{#if item.t === 'turn_started'}<strong>Turn started</strong><p>{item.trigger}</p>{:else if item.t === 'tool_started'}<strong>{item.tool}</strong><p>Tool started</p>{:else if item.t === 'schedule_fired'}<strong>Routine fired</strong><p>Schedule #{item.schedule}</p>{:else if item.t === 'turn_finished'}<strong class:danger={!item.ok}>{item.ok ? 'Turn completed' : 'Turn failed'}</strong>{/if}<small>Run #{item.run}</small></div></article>
      {/each}
    </div>
    {#if !activity.length}<div class="panel-empty"><Icon name="terminal" size={28} /><h3>Listening for activity</h3><p>Turns, tools, and scheduled work appear here as they happen.</p></div>{/if}
  {/if}
</div>

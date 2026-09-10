<script lang="ts">
  import type { Event, Op } from '../shared/api';
  import Markdown from '../shared/Markdown.svelte';
  import Icon from '../shared/Icon.svelte';
  let { plan, onop, onerror }: { plan: Extract<Event, { type: 'PlanProposed' }>; onop: (op: Op) => Promise<void>; onerror: (message: string) => void } = $props();
  let feedback = $state('');
  let sending = $state(false);
  let error = $state('');
  async function resolve(approved: boolean) {
    sending = true;
    error = '';
    try {
      await onop({ type: 'ResolvePlan', call: plan.call, decision: approved ? { type: 'Approve' } : { type: 'Reject', feedback } });
    } catch (failure) {
      error = String(failure);
    } finally {
      sending = false;
    }
  }
</script>

<section class="request-card plan-card">
  <div class="eyebrow"><Icon name="check" /> Plan ready for review</div>
  <p class="muted small path">{plan.path}</p>
  <Markdown text={plan.plan} {onerror} />
  <label class="field-label" for={`feedback-${plan.call}`}>Feedback</label>
  <textarea id={`feedback-${plan.call}`} class="text-field" bind:value={feedback} rows="2" placeholder="What should change?" disabled={sending}></textarea>
  {#if error}<p class="inline-error" role="alert">{error}</p>{/if}
  <div class="button-row"><button class="primary" disabled={sending} onclick={() => resolve(true)}><Icon name="check" /> Approve plan</button><button class="secondary" disabled={sending || !feedback.trim()} onclick={() => resolve(false)}>Request changes</button></div>
</section>

<script lang="ts">
  import type { Event, Op } from '../shared/api';
  import Icon from '../shared/Icon.svelte';
  let { ask, onop }: { ask: Extract<Event, { type: 'AskStarted' }>; onop: (op: Op) => Promise<void> } = $props();
  let choices = $state<Record<number, string[]>>({});
  let free = $state<Record<number, string>>({});
  let sending = $state(false);
  let error = $state('');
  function choose(index: number, label: string, multiple: boolean) {
    const selected = choices[index] ?? [];
    choices[index] = multiple ? selected.includes(label) ? selected.filter(value => value !== label) : [...selected, label] : [label];
    if (!multiple) free[index] = '';
  }
  async function submit(skip = false) {
    sending = true;
    error = '';
    try {
      await onop({ type: 'Answer', id: ask.id, call: ask.call, answers: ask.questions.map((_, index) => skip ? '' : [...choices[index] ?? [], ...(free[index]?.trim() ? [free[index].trim()] : [])].join(', ')) });
    } catch (failure) {
      error = String(failure);
    } finally {
      sending = false;
    }
  }
</script>

<section class="request-card">
  <div class="eyebrow"><Icon name="agent" /> Your input is needed</div>
  {#each ask.questions as question, index}
    <fieldset disabled={sending}>
      <legend>{question.question}</legend>
      {#if question.multiple}<p class="muted small">Choose any that apply, or add your own answer.</p>{/if}
      <div class="answer-options">
        {#each question.options ?? [] as option}
          <label class:selected={(choices[index] ?? []).includes(option.label)} class="answer-option">
            <input type={question.multiple ? 'checkbox' : 'radio'} name={`ask-${ask.call}-${index}`} checked={(choices[index] ?? []).includes(option.label)} onchange={() => choose(index, option.label, question.multiple ?? false)} />
            <span><strong>{option.label}</strong>{#if option.description}<small>{option.description}</small>{/if}</span>
          </label>
        {/each}
      </div>
      <input class="text-field" aria-label={`Your answer to ${question.question}`} placeholder={question.options?.length ? 'Or write your own answer…' : 'Your answer…'} value={free[index] ?? ''} oninput={event => { free[index] = event.currentTarget.value; if (!question.multiple) choices[index] = []; }} />
    </fieldset>
  {/each}
  {#if error}<p class="inline-error" role="alert">{error}</p>{/if}
  <div class="button-row"><button class="primary" disabled={sending} onclick={() => submit()}>{sending ? 'Sending…' : 'Send answers'}<Icon name="arrow" /></button><button class="quiet" disabled={sending} onclick={() => submit(true)}>Skip</button></div>
</section>

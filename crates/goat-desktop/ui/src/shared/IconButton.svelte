<script lang="ts">
  import type { ClassValue } from 'svelte/elements';
  import Icon from './Icon.svelte';
  import type { IconName } from './icons';

  let {
    icon,
    label,
    hint,
    disabled = false,
    expanded,
    haspopup,
    onclick,
    class: klass,
  }: {
    icon: IconName;
    label: string;
    hint?: string;
    disabled?: boolean;
    expanded?: boolean;
    haspopup?: boolean;
    onclick?: (event: MouseEvent) => void;
    class?: ClassValue;
  } = $props();
</script>

<button
  type="button"
  class={[
    'inline-flex size-[26px] shrink-0 items-center justify-center',
    'text-fg-3 transition-[color,opacity] duration-100 hover:text-fg-1 active:text-fg-2',
    'disabled:text-disabled disabled:cursor-default',
    klass,
  ]}
  aria-label={label}
  aria-expanded={expanded}
  aria-haspopup={haspopup ? 'menu' : undefined}
  title={hint ?? label}
  {disabled}
  {onclick}
>
  <Icon name={icon} />
</button>

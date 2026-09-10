<script lang="ts">
  import type { ClassValue } from 'svelte/elements';
  import { paths, type IconName } from './icons';

  let { name, size = 16, class: klass }: { name: IconName; size?: number; class?: ClassValue } = $props();
  const shapes = $derived.by(() => {
    const value = paths[name];
    return typeof value === 'string' ? [value] : value;
  });
</script>

<svg
  class={klass}
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill={name === 'dots' ? 'currentColor' : 'none'}
  stroke={name === 'dots' ? 'none' : 'currentColor'}
  stroke-width="1.5"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>{#each shapes as shape (shape)}<path d={shape} />{/each}</svg>

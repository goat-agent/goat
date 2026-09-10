<script lang="ts">
  import Icon from './Icon.svelte';
  import IconButton from './IconButton.svelte';
  import type { IconName } from './icons';

  let {
    label,
    icon,
    disclosure,
    open = false,
    indent = false,
    active = false,
    busy = false,
    menu,
    onclick,
    onmenu,
    oncontext,
  }: {
    label: string;
    icon?: IconName;
    disclosure?: boolean;
    open?: boolean;
    indent?: boolean;
    active?: boolean;
    busy?: boolean;
    menu?: string;
    onclick?: () => void;
    onmenu?: (anchor: HTMLElement) => void;
    oncontext?: (x: number, y: number) => void;
  } = $props();

  const box = 'group relative flex h-[30px] w-full items-center gap-[9px] rounded-md pr-1.5 text-left transition-colors duration-100';
  const tone = $derived(active
    ? 'bg-fill-active text-fg-1'
    : disclosure
      ? 'text-fg-1 hover:bg-fill-hover'
      : 'text-fg-2 hover:bg-fill-hover hover:text-fg-hover');
  const glyph = $derived(active ? 'text-fg-1' : disclosure ? 'text-fg-3 group-hover:text-fg-hover' : 'text-fg-3 group-hover:text-fg-hover');

  function context(event: MouseEvent) {
    if (!oncontext) return;
    event.preventDefault();
    oncontext(event.clientX, event.clientY);
  }
</script>

<div class={[box, tone, indent ? 'pl-[35px]' : 'pl-2.5']}>
  <button
    type="button"
    class="flex min-w-0 flex-1 items-center gap-[9px] text-left after:absolute after:inset-0 after:rounded-md"
    aria-expanded={disclosure ? open : undefined}
    aria-current={active ? 'page' : undefined}
    onclick={() => onclick?.()}
    oncontextmenu={context}
  >
    {#if icon}
      <span class={['relative size-4 shrink-0', glyph]}>
        <Icon name={icon} class="absolute inset-0 transition-opacity duration-100 {disclosure ? 'group-hover:opacity-0' : ''}" />
        {#if disclosure}
          <Icon
            name="chevron"
            class={['absolute inset-0 opacity-0 transition-[opacity,transform] duration-150 ease-fold group-hover:opacity-100', open && 'rotate-90']}
          />
        {/if}
      </span>
    {/if}
    <span class="min-w-0 flex-1 truncate">{label}</span>
  </button>

  {#if menu || busy}
    <span class="relative z-10 -mr-1 grid size-[26px] shrink-0 place-items-center">
      {#if busy}
        <span
          class="col-start-1 row-start-1 size-3 animate-spin-fast rounded-full border-[1.5px] border-fill-active border-t-accent transition-opacity duration-100 group-hover:opacity-0"
          role="status"
          aria-label="Running"
        ></span>
      {/if}
      {#if menu}
        <IconButton
          icon="dots"
          label={menu}
          haspopup
          class="col-start-1 row-start-1 size-[26px] opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
          onclick={(event) => onmenu?.(event.currentTarget as HTMLElement)}
        />
      {/if}
    </span>
  {/if}
</div>

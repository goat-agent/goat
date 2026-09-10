<script lang="ts">
  import IconButton from '../shared/IconButton.svelte';

  let {
    collapsed,
    canBack,
    canForward,
    ontoggle,
    onback,
    onforward,
  }: {
    collapsed: boolean;
    canBack: boolean;
    canForward: boolean;
    ontoggle: () => void;
    onback: () => void;
    onforward: () => void;
  } = $props();
</script>

<div class="flex h-11 shrink-0 items-center gap-0.5 pr-2.5 pl-[91px]" data-tauri-drag-region>
  <IconButton
    icon="sidebar"
    label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
    hint={`${collapsed ? 'Expand' : 'Collapse'} sidebar · ⌘B`}
    onclick={ontoggle}
  />
  <IconButton icon="back" label="Back" hint="Back · ⌘[" disabled={!canBack} onclick={onback} />
  <IconButton icon="forward" label="Forward" hint="Forward · ⌘]" disabled={!canForward} onclick={onforward} />
</div>

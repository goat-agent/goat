<script lang="ts">
  import { markdown, openMarkdownLink } from './markdown';
  import { errorText } from './ipc';
  let { text, onerror }: { text: string; onerror: (message: string) => void } = $props();
  const html = $derived(markdown(text));
  function links(node: HTMLElement) {
    const click = (event: MouseEvent) => { void openMarkdownLink(event).catch(error => onerror(errorText(error))); };
    node.addEventListener('click', click);
    return { destroy: () => node.removeEventListener('click', click) };
  }
</script>

<div class="markdown" use:links>{@html html}</div>

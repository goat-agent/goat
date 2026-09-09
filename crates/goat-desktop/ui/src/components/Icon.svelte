<script lang="ts">
  const paths = {
    code: 'm8 7-5 5 5 5m8-10 5 5-5 5m-3-13-2 16',
    agent: 'M8 4h8a4 4 0 0 1 4 4v7a4 4 0 0 1-4 4H8l-4 3V8a4 4 0 0 1 4-4ZM8 10h.01M16 10h.01M9 15h6',
    folder: 'M3 7V5a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Zm0 1h18',
    plus: 'M12 5v14M5 12h14',
    search: 'm21 21-5-5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0Z',
    chevron: 'm9 5 7 7-7 7',
    down: 'm6 9 6 6 6-6',
    send: 'M12 19V5m-6 6 6-6 6 6',
    image: 'M4 3h16a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Zm-1 13 6-6 12 10M16 7h.01',
    terminal: 'm4 5 6 7-6 7m9 0h7',
    stop: 'M6 6h12v12H6Z',
    close: 'm6 6 12 12M6 18 18 6',
    check: 'm4 12 5 5L20 6',
    settings: 'M4 7h16M4 17h16M9 4v6M15 14v6',
    git: 'M6 3v12a4 4 0 0 0 8 0V9m0 0a4 4 0 0 0 4-4M6 3h.01M6 21h.01M18 3h.01',
    external: 'M14 3h7v7m0-7L10 14M10 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-5',
    clock: 'M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0ZM12 6v6l4 2',
    cpu: 'M6 6h12v12H6ZM9 9h6v6H9ZM9 2v4m6-4v4M9 18v4m6-4v4M2 9h4m-4 6h4m12-6h4m-4 6h4',
    panel: 'M3 3h18v18H3Zm12 0v18',
    refresh: 'M20 7a9 9 0 1 0 1 9M20 2v6h-6',
    paperclip: 'm8 12 6-6a3 3 0 0 1 4 4l-9 9a5 5 0 0 1-7-7L12 2m-7 13 9-9',
    shield: 'm12 2 9 4v6c0 5-9 10-9 10S3 17 3 12V6l9-4Zm-5 10 3 3 7-7',
    arrow: 'M4 12h16m-6-6 6 6-6 6',
  };
  let { name, size = 16 }: { name: keyof typeof paths; size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>

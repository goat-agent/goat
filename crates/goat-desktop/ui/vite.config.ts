import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  clearScreen: false,
  server: { host: '127.0.0.1', port: 5173, strictPort: true },
  build: { target: 'safari17', sourcemap: false },
});

import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [sveltekit()],
  // Never inline assets as data: URIs. Vite's default 4 KiB limit inlined a woff2 font into app.css, and
  // font-src 'self' (security-headers.json) blocks data: fonts. Every font must be a file under /_app/immutable.
  build: { assetsInlineLimit: 0 },
  server: {
    proxy: {
      '/api': { target: 'http://localhost:8000', changeOrigin: false },
      '/auth/v1': { target: 'http://localhost:8000', changeOrigin: false }
    }
  },
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
  // Node globals, but Svelte compiles for the client (see src/test/env.ts), so $effect.root works.
  test: { include: ['src/**/*.test.ts'], environment: './src/test/env.ts', setupFiles: ['src/test/setup.ts'] }
});

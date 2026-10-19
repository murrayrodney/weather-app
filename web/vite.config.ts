import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [sveltekit()],
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

import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

export default {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({ pages: 'build', assets: 'build', fallback: '200.html', precompress: false, strict: true }),
    output: { preloadStrategy: 'modulepreload' },
    csp: { mode: 'hash', directives: { 'script-src': ['self'] } } // the header carries every other directive (security-headers.json)
  }
};

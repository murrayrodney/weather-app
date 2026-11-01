<script lang="ts">
  // Self-hosted fonts (CSP font-src 'self'): Vite bundles the woff2 files under /_app/immutable.
  import '@fontsource-variable/manrope';
  import '@fontsource/jetbrains-mono/500.css';
  import '../app.css';
  import { onMount } from 'svelte';
  import type { Snippet } from 'svelte';
  import Footer from '$lib/components/Footer.svelte';
  import { initAuth } from '$lib/auth/session.svelte';

  let { children }: { children: Snippet } = $props();

  onMount(() => {
    void initAuth();
  });
</script>

{@render children()}
<Footer />

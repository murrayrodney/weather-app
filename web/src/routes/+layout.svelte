<script lang="ts">
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

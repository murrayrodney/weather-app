<script lang="ts">
  import { goto } from '$app/navigation';
  import type { Snippet } from 'svelte';
  import { signOut } from '$lib/auth/session.svelte';

  let { children }: { children: Snippet } = $props();

  async function out() {
    const { error, kept } = await signOut('local');
    if (error && kept) {
      problem = 'Could not sign out. Check your connection and try again.';
      return;
    }
    await goto(error ? '/login?reason=revoke_failed' : '/login');
  }
  let problem = $state('');
</script>

<nav class="site-nav" aria-label="Main">
  <a class="brand" href="/">Family Weather</a>
  <a href="/">Dashboard</a>
  <a href="/follows">Follows</a>
  <a href="/connect">Connect station</a>
  <a href="/account">Account</a>
  <span class="spacer"></span>
  <button type="button" class="secondary" onclick={out}>Sign out</button>
</nav>

{#if problem}<p class="banner warn" role="alert">{problem}</p>{/if}
{@render children()}

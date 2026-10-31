<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import type { Snippet } from 'svelte';
  import { signOut } from '$lib/auth/session.svelte';
  import ThemeToggle from '$lib/components/ThemeToggle.svelte';

  let { children }: { children: Snippet } = $props();

  const links = [
    { href: '/', label: 'Dashboard' },
    { href: '/follows', label: 'Follows' },
    { href: '/connect', label: 'Connect station' },
    { href: '/account', label: 'Account' }
  ];

  /** The link for the current route (exact for the dashboard, the link or a sub-path otherwise); none on other pages. */
  function isCurrent(href: string, path: string): boolean {
    if (href === '/') return path === '/';
    return path === href || path.startsWith(href + '/');
  }

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
  <a class="brand" href="/"><span class="brand-mark" aria-hidden="true"></span>Family Weather</a>
  {#each links as link (link.href)}
    <a href={link.href} aria-current={isCurrent(link.href, page.url.pathname) ? 'page' : undefined}>{link.label}</a>
  {/each}
  <span class="spacer"></span>
  <ThemeToggle />
  <button type="button" class="secondary" onclick={out}>Sign out</button>
</nav>

{#if problem}<p class="banner warn" role="alert">{problem}</p>{/if}
{@render children()}

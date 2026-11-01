<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { clearPreviousSession, createSetPasswordFlow, takeTokenFromLocation, validateNewPassword } from '$lib/auth/password';

  let hasLink = $state<boolean | null>(null);
  let password = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let message = $state('');
  // The token lives only in this closure (never in the DOM, storage, logs or the URL).
  let flow: ReturnType<typeof createSetPasswordFlow> | null = null;
  let priorSignOut: Promise<void> = Promise.resolve();

  onMount(() => {
    // First thing, synchronously: read the fragment, then strip it from the address bar.
    const token = takeTokenFromLocation();
    if (!token) {
      hasLink = false;
      return;
    }
    flow = createSetPasswordFlow(token);
    hasLink = true;
    // A different signed-in user's session would be replaced by verifyOtp: clear it (and their caches) first.
    priorSignOut = clearPreviousSession().then(() => {});
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy || !flow) return;
    const problem = validateNewPassword(password, confirm);
    if (problem) {
      message = problem;
      return;
    }
    busy = true;
    message = '';
    await priorSignOut;
    const r = await flow.submit(password);
    busy = false;
    if (r.ok) {
      password = '';
      confirm = '';
      await goto('/', { replaceState: true });
    } else {
      message = r.message;
    }
  }
</script>

<main class="page narrow">
  <h1>Choose a password</h1>
  {#if hasLink === false}
    <p>This page needs a link from Rodney. Ask for a new one.</p>
    <p><a href="/login">Go to sign in</a></p>
  {:else if hasLink}
    <form onsubmit={submit} class="card">
      <label class="field">
        <span>New password (at least 12 characters)</span>
        <input type="password" name="new-password" autocomplete="new-password" required bind:value={password} />
      </label>
      <label class="field">
        <span>Confirm password</span>
        <input type="password" name="confirm-password" autocomplete="new-password" required bind:value={confirm} />
      </label>
      <button type="submit" disabled={busy}>Set password</button>
      {#if message}<p class="error-text" role="alert">{message}</p>{/if}
    </form>
  {/if}
</main>

<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { safeNext } from '$lib/auth/next';
  import { SIGN_IN_MESSAGES, signIn } from '$lib/auth/login';

  let email = $state('');
  let password = $state('');
  let busy = $state(false);
  let message = $state('');
  let failures = $state(0);
  let locked = $state(false);
  const LOCK_MS = 30000;

  const expired = $derived(page.url.searchParams.get('reason') === 'expired');
  const revokeFailed = $derived(page.url.searchParams.get('reason') === 'revoke_failed');

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy || locked) return;
    busy = true;
    message = '';
    const result = await signIn(email, password);
    busy = false;
    if (result === 'ok') {
      password = '';
      // `next` is read from the current URL at the moment of success; no storage.
      await goto(safeNext(new URL(location.href).searchParams.get('next')), { replaceState: true });
      return;
    }
    message = SIGN_IN_MESSAGES[result];
    if (result === 'failed') {
      failures += 1;
      // Client-side convenience only (not a security control): pause the button for 30 s after 5 failures in a row.
      if (failures >= 5) {
        locked = true;
        failures = 0;
        setTimeout(() => {
          locked = false;
        }, LOCK_MS);
      }
    } else {
      failures = 0;
    }
  }
</script>

<main class="page narrow">
  <h1>Sign in</h1>
  {#if expired}<p class="banner" role="status">Your session expired. Sign in again.</p>{/if}
  {#if revokeFailed}
    <p class="banner warn" role="status" data-testid="revoke-failed">
      You are signed out on this device, but we could not reach the server to end your other sessions. Sign in and choose "Sign out everywhere" again if you need to.
    </p>
  {/if}
  <form onsubmit={submit} class="card">
    <label class="field">
      <span>Email</span>
      <input type="email" name="email" autocomplete="username" required bind:value={email} />
    </label>
    <label class="field">
      <span>Password</span>
      <input type="password" name="password" autocomplete="current-password" required bind:value={password} />
    </label>
    <button type="submit" disabled={busy || locked}>Sign in</button>
    {#if locked}<p class="error-text" role="status" data-testid="login-locked">Too many failed attempts. Wait 30 seconds before trying again.</p>{/if}
    {#if message}<p class="error-text" role="alert">{message}</p>{/if}
  </form>
  <p class="muted small">Forgot your password or don't have one yet? Ask Rodney for a new link.</p>
</main>

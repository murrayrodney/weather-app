<script lang="ts">
  import { goto } from '$app/navigation';
  import { supabase } from '$lib/auth/supabase';
  import { changePassword, validateNewPassword } from '$lib/auth/password';
  import { signOut } from '$lib/auth/session.svelte';
  import { onMount } from 'svelte';

  let { data } = $props();

  let email = $state('');
  let password = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let message = $state('');
  let done = $state(false);

  onMount(async () => {
    const { data: u } = await supabase.auth.getUser();
    email = u.user?.email ?? '';
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    done = false;
    const problem = validateNewPassword(password, confirm, email);
    if (problem) {
      message = problem;
      return;
    }
    busy = true;
    message = '';
    const r = await changePassword(password);
    busy = false;
    if (r.ok) {
      done = true;
      message = 'Password changed.';
      password = '';
      confirm = '';
    } else {
      message = r.message;
    }
  }

  let signOutProblem = $state('');
  async function out(scope: 'local' | 'global') {
    signOutProblem = '';
    const { error, kept } = await signOut(scope);
    if (error && kept) {
      signOutProblem = 'Could not sign out. You are still signed in here. Try again.';
      return;
    }
    // supabase-js drops the local session even when the server revoke fails: say so on the login page.
    await goto(error ? '/login?reason=revoke_failed' : '/login');
  }
</script>

<main class="page narrow">
  <h1>Account</h1>
  <div class="card">
    <p><strong>{data.me.display_name}</strong></p>
    {#if email}<p class="muted">{email}</p>{/if}
  </div>

  <h2>Change password</h2>
  <form onsubmit={submit} class="card">
    <label class="field">
      <span>New password (at least 12 characters)</span>
      <input type="password" name="new-password" autocomplete="new-password" required bind:value={password} />
    </label>
    <label class="field">
      <span>Confirm password</span>
      <input type="password" name="confirm-password" autocomplete="new-password" required bind:value={confirm} />
    </label>
    <button type="submit" disabled={busy}>Change password</button>
    {#if message}<p class={done ? 'ok-text' : 'error-text'} role="status">{message}</p>{/if}
  </form>

  <h2>Sign out</h2>
  <div class="inline-form">
    <button type="button" class="secondary" onclick={() => out('local')}>Sign out</button>
    <button type="button" class="secondary" onclick={() => out('global')}>Sign out everywhere</button>
  </div>
  {#if signOutProblem}<p class="error-text" role="alert" data-testid="signout-problem">{signOutProblem}</p>{/if}
</main>

<script lang="ts">
  import { requestFollow, searchUsers } from '$lib/api/endpoints';
  import { ApiError } from '$lib/api/errors';
  import type { UserHit } from '$lib/api/types';
  import { callAction, handleAuthError } from '$lib/auth/redirect';

  let { onrequested }: { onrequested?: () => void } = $props();

  let q = $state('');
  let hits = $state<UserHit[] | null>(null);
  let message = $state('');
  let disabled = $state(false);
  let sent = $state<string[]>([]);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let controller: AbortController | undefined;

  function schedule() {
    clearTimeout(timer);
    controller?.abort();
    const term = q.trim();
    if (term.length < 2) {
      hits = null;
      message = '';
      return;
    }
    timer = setTimeout(() => void run(term), 300);
  }

  async function run(term: string) {
    controller?.abort();
    const mine = new AbortController();
    controller = mine;
    try {
      const r = await searchUsers(term, mine.signal);
      if (mine.signal.aborted) return;
      hits = r;
      message = '';
    } catch (err) {
      if (mine.signal.aborted) return;
      if (handleAuthError(err)) return;
      if (err instanceof ApiError && err.code === 'rate_limited') {
        const n = err.retryAfterSec ?? 10;
        message = `Too many searches. Try again in ${n} seconds.`;
        disabled = true;
        setTimeout(() => (disabled = false), n * 1000);
      } else {
        message = 'Search failed. Try again.';
      }
    }
  }

  async function request(h: UserHit) {
    const r = await callAction(() => requestFollow(h.id));
    if (r.ok) {
      sent = [...sent, h.id];
      onrequested?.();
      return;
    }
    if (r.handled) return;
    if (r.error instanceof ApiError && r.error.status === 404) {
      message = "That person isn't available.";
      void run(q.trim());
    } else message = 'Could not send the request. Try again.';
  }

  const rowState = (h: UserHit) => (sent.includes(h.id) ? 'pending' : h.follow_status);
</script>

<section class="card user-search">
  <h2>Find people</h2>
  <label class="field">
    <span>Find a person by name</span>
    <input type="text" autocomplete="off" {disabled} bind:value={q} oninput={schedule} maxlength="40" />
  </label>
  {#if q.trim().length < 2}<p class="small muted">Type at least 2 characters.</p>{/if}
  <div aria-live="polite">
    {#if message}<p class="error-text">{message}</p>{/if}
    {#if hits && hits.length === 0}
      <p class="muted">No one found. Names are matched from the start; check the spelling.</p>
    {:else if hits}
      <ul class="plain" data-testid="search-results">
        {#each hits as h (h.id)}
          <li>
            <span>{h.display_name}</span>
            {#if rowState(h) === 'none' || rowState(h) === 'declined' || rowState(h) === 'revoked'}
              <button type="button" onclick={() => request(h)}>Request to follow</button>
            {:else if rowState(h) === 'pending'}
              <span class="badge pending">{sent.includes(h.id) ? 'Pending' : 'Request sent'}</span>
            {:else}
              <span class="badge approved">Following</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</section>

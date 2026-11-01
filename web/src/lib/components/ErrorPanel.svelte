<script lang="ts">
  import { ApiError, userMessage } from '$lib/api/errors';

  let { error, onretry }: { error: unknown; onretry?: () => void } = $props();

  // Honour Retry-After: the Retry button stays disabled until the wait is over.
  let waiting = $state(false);
  $effect(() => {
    const secs = error instanceof ApiError ? error.retryAfterSec : undefined;
    if (!secs) {
      waiting = false;
      return;
    }
    waiting = true;
    const t = setTimeout(() => (waiting = false), secs * 1000);
    return () => clearTimeout(t);
  });
</script>

<div class="banner warn" role="alert">
  <p class="error-text">{userMessage(error)}</p>
  {#if onretry}
    <button type="button" class="secondary" disabled={waiting} onclick={onretry}>Retry</button>
  {/if}
</div>

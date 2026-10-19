<script lang="ts">
  import { decideFollow, getFollows } from '$lib/api/endpoints';
  import { ApiError, userMessage } from '$lib/api/errors';
  import type { FollowRow } from '$lib/api/types';
  import { callAction } from '$lib/auth/redirect';
  import ErrorPanel from '$lib/components/ErrorPanel.svelte';
  import FollowItem from '$lib/components/FollowItem.svelte';
  import LoadingBlock from '$lib/components/LoadingBlock.svelte';
  import UserSearch from '$lib/components/UserSearch.svelte';
  import { createResource } from '$lib/data/resource.svelte';

  const follows = createResource((signal) => getFollows(signal));
  let message = $state('');

  const followers = $derived(follows.data?.followers ?? []);
  const pending = $derived(followers.filter((f) => f.status === 'pending'));
  const approved = $derived(followers.filter((f) => f.status === 'approved'));
  const closed = $derived(followers.filter((f) => f.status === 'declined' || f.status === 'revoked'));

  async function act(action: 'approve' | 'decline' | 'revoke', row: FollowRow) {
    message = '';
    const r = await callAction(() => decideFollow(row.user.id, action));
    if (!r.ok && !r.handled) {
      message = r.error instanceof ApiError && r.error.status === 404 ? 'That request is no longer pending.' : userMessage(r.error);
    }
    if (!r.ok && r.handled) return;
    follows.reload();
  }
</script>

<main class="page narrow">
  <h1>Follows</h1>
  {#if follows.status === 'loading' && !follows.data}
    <LoadingBlock />
  {:else if follows.status === 'error' && !follows.data}
    <ErrorPanel error={follows.error} onretry={() => follows.reload()} />
  {:else}
    {#if message}<p class="error-text" role="alert">{message}</p>{/if}
    <h2>People who follow me</h2>
    <p class="small muted">Approving shares all of your stations.</p>
    {#if followers.length === 0}<p class="muted">Nobody has asked yet.</p>{/if}
    <ul class="plain" data-testid="followers">
      {#each pending as r (r.user.id)}<FollowItem row={r} direction="follower" onact={act} />{/each}
      {#each approved as r (r.user.id)}<FollowItem row={r} direction="follower" onact={act} />{/each}
    </ul>
    {#if closed.length}
      <details>
        <summary>Declined or revoked ({closed.length})</summary>
        <ul class="plain">
          {#each closed as r (r.user.id)}<FollowItem row={r} direction="follower" onact={act} />{/each}
        </ul>
      </details>
    {/if}

    <h2>People I follow</h2>
    {#if (follows.data?.following ?? []).length === 0}<p class="muted">You are not following anyone yet.</p>{/if}
    <ul class="plain" data-testid="following">
      {#each follows.data?.following ?? [] as r (r.user.id)}<FollowItem row={r} direction="following" />{/each}
    </ul>
    <UserSearch onrequested={() => follows.reload()} />
  {/if}
</main>

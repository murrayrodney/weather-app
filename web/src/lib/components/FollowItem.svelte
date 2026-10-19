<script lang="ts">
  import type { FollowRow } from '$lib/api/types';
  import { formatDateLong } from '$lib/transforms/dates';

  let {
    row,
    direction,
    onact
  }: { row: FollowRow; direction: 'follower' | 'following'; onact?: (action: 'approve' | 'decline' | 'revoke', row: FollowRow) => void } = $props();

  let confirming = $state(false);
</script>

<li data-testid="follow-row" data-name={row.user.display_name}>
  <strong>{row.user.display_name}</strong>
  <span class={'badge ' + row.status}>{row.status}</span>
  {#if direction === 'follower'}
    <span class="small muted">Requested {formatDateLong(row.requested_at.slice(0, 10))}</span>
    {#if row.status === 'pending'}
      <button type="button" onclick={() => onact?.('approve', row)}>Approve</button>
      <button type="button" class="secondary" onclick={() => onact?.('decline', row)}>Decline</button>
    {:else if row.status === 'approved'}
      {#if !confirming}
        <button type="button" class="danger" onclick={() => (confirming = true)}>Revoke</button>
      {:else}
        <span class="small">Stop sharing your stations with {row.user.display_name}?</span>
        <button type="button" class="danger" onclick={() => onact?.('revoke', row)}>Confirm revoke</button>
        <button type="button" class="secondary" onclick={() => (confirming = false)}>Cancel</button>
      {/if}
    {/if}
  {/if}
</li>

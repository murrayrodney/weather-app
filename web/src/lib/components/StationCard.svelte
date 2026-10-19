<script lang="ts">
  import type { Station } from '$lib/api/types';
  import { STALE_AFTER_MIN } from '$lib/config';
  import { formatInZone } from '$lib/transforms/dates';

  let { station, credentialStatus = null, now = Date.now() }: { station: Station; credentialStatus?: string | null; now?: number } = $props();

  const last = $derived(station.latest_observation_at);
  const stale = $derived(last === null || now - Date.parse(last) > STALE_AFTER_MIN * 60000);
  const note = $derived(
    station.is_own && credentialStatus === 'rate_limited' ? 'Updates are delayed.' : stale && credentialStatus !== 'invalid' ? 'No recent data.' : null
  );
</script>

<div class="station-head">
  <a class="name" href={'/stations/' + station.id}>{station.name}</a>
  {#if !station.is_own}<span class="muted small">{station.owner.display_name}</span>{/if}
  <span class="muted small">
    {#if last}Latest reading: {formatInZone(last, station.timezone)}{:else}No readings yet{/if}
  </span>
  {#if note}<span class="small muted">{note}</span>{/if}
</div>

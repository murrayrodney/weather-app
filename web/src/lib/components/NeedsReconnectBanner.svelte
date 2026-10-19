<script lang="ts">
  import { formatInZone } from '$lib/transforms/dates';

  let { lastObservationAt, timezone }: { lastObservationAt: string | null; timezone: string } = $props();
  const last = $derived(lastObservationAt ? formatInZone(lastObservationAt, timezone) : 'none yet');
</script>

<div class="banner warn">
  Data collection paused: this station needs reconnecting. Last data: {last}.
  <a href="/connect?reconnect=1">Reconnect</a>
</div>

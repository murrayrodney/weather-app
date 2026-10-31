<script lang="ts">
  import type { Forecast } from '$lib/api/types';
  import { formatInZone } from '$lib/transforms/dates';

  let { forecast }: { forecast: Forecast } = $props();
</script>

{#if forecast.stale}
  <p class="banner warn" data-testid="stale-note">Showing the forecast saved at {formatInZone(forecast.generated_at, forecast.timezone)}; refreshing failed.</p>
{/if}
{#if !forecast.ensemble}<p class="state-note" data-testid="no-ensemble">Uncertainty range unavailable right now.</p>{/if}
{#if !forecast.deterministic}<p class="state-note">The single-model forecast line is unavailable right now.</p>{/if}

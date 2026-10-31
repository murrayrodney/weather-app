<script lang="ts">
  import { getForecast } from '$lib/api/endpoints';
  import type { Station } from '$lib/api/types';
  import ForecastStack from '$lib/charts/ForecastStack.svelte';
  import { createResource } from '$lib/data/resource.svelte';
  import ErrorPanel from './ErrorPanel.svelte';
  import LoadingBlock from './LoadingBlock.svelte';

  let { station }: { station: Station } = $props();

  // One /forecast call per station (the API caches 30 minutes; the client cache absorbs tab switches).
  const forecast = createResource((signal) => getForecast(station.id, signal));
</script>

<section class="card forecast-row" data-testid="forecast-row">
  <h2 class="row-title"><a href={'/stations/' + station.id}>{station.name}</a></h2>
  {#if forecast.status === 'loading'}
    <LoadingBlock label="Loading forecast..." />
  {:else if forecast.status === 'error'}
    <ErrorPanel error={forecast.error} onretry={() => forecast.reload()} />
  {:else if forecast.data}
    <ForecastStack forecast={forecast.data} compact />
  {/if}
</section>

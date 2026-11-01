<script lang="ts">
  import { getForecast, getForecastChanges } from '$lib/api/endpoints';
  import { ApiError } from '$lib/api/errors';
  import type { Station } from '$lib/api/types';
  import ForecastChangesChart from '$lib/charts/ForecastChangesChart.svelte';
  import ForecastChart from '$lib/charts/ForecastChart.svelte';
  import { FEATURES, FORECAST_CHANGES_DAYS } from '$lib/config';
  import { createResource } from '$lib/data/resource.svelte';
  import { addDays, daysBetween, localDate } from '$lib/transforms/dates';
  import ErrorPanel from './ErrorPanel.svelte';
  import LoadingBlock from './LoadingBlock.svelte';

  // The forecast and forecast-change sections of the station page (the first two sections, anchors #forecast and #changes).
  let { station }: { station: Station } = $props();

  const today = $derived(localDate(station.timezone));

  let fromInput = $state('');
  let toInput = $state('');
  $effect(() => {
    if (!fromInput) {
      fromInput = today;
      toInput = addDays(today, FORECAST_CHANGES_DAYS - 1);
    }
  });
  const from = $derived(fromInput || today);
  const to = $derived(toInput || addDays(today, FORECAST_CHANGES_DAYS - 1));
  const rangeProblem = $derived.by(() => {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(from) || !/^\d{4}-\d{2}-\d{2}$/.test(to)) return 'Pick a valid date range.';
    // Same inclusive rule as the API (routes/forecast.rs): (to - from) + 1 > 14 is rejected.
    const days = daysBetween(from, to) + 1;
    if (days < 1 || days > 14) return 'Pick up to 14 days.';
    if (from < '2024-01-01' || to > addDays(today, 15)) return 'Pick up to 14 days.';
    return null;
  });

  // The two forecast routes share one per-user limiter (20/min, burst 10): load them one after the other.
  const forecast = createResource(async (signal) => {
    const id = station.id;
    if (!FEATURES.forecast) return null;
    return getForecast(id, signal);
  });
  const changes = createResource(async (signal) => {
    const id = station.id;
    const f = from;
    const t = to;
    const problem = rangeProblem;
    const fcStatus = forecast.status;
    if (!FEATURES.forecast || problem || fcStatus === 'loading') return null;
    return getForecastChanges(id, f, t, signal);
  });
  const pastNullProb = $derived(
    from < today && (changes.data?.days ?? []).some((d) => d.date < today && d.prob_precip_max.some((v) => v === null))
  );
</script>

{#if !FEATURES.forecast}
  <section id="forecast" class="station-section">
    <h2>Forecast</h2>
    <p>Forecasts are not enabled.</p>
  </section>
{:else}
  <section id="forecast" class="station-section" data-testid="forecast-section">
    <h2>Forecast</h2>
    {#if forecast.status === 'loading'}
      <LoadingBlock label="Loading forecast..." />
    {:else if forecast.status === 'error'}
      <ErrorPanel error={forecast.error} onretry={() => forecast.reload()} />
    {:else if forecast.data}
      <ForecastChart forecast={forecast.data} />
    {/if}
  </section>

  <section id="changes" class="station-section" data-testid="changes-section">
    <h2>How the forecast changed</h2>
    <div class="controls">
      <label class="small">From <input type="date" min="2024-01-01" bind:value={fromInput} placeholder={today} /></label>
      <label class="small">To <input type="date" bind:value={toInput} placeholder={to} /></label>
    </div>
    {#if rangeProblem}
      <p class="error-text" role="alert">{rangeProblem}</p>
    {:else if forecast.status === 'loading' || changes.status === 'loading'}
      <LoadingBlock label="Loading forecast changes..." />
    {:else if changes.status === 'error' && changes.error instanceof ApiError && changes.error.status === 422}
      <p class="error-text" role="alert">Pick up to 14 days.</p>
    {:else if changes.status === 'error'}
      <ErrorPanel error={changes.error} onretry={() => changes.reload()} />
    {:else if changes.data}
      <ForecastChangesChart changes={changes.data} {pastNullProb} />
    {/if}
  </section>
{/if}

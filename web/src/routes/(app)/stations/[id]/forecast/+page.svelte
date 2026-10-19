<script lang="ts">
  import { page } from '$app/state';
  import { getForecast, getForecastChanges, getStation } from '$lib/api/endpoints';
  import { ApiError } from '$lib/api/errors';
  import { isUuid } from '$lib/api/ids';
  import ForecastChangesChart from '$lib/charts/ForecastChangesChart.svelte';
  import ForecastChart from '$lib/charts/ForecastChart.svelte';
  import ErrorPanel from '$lib/components/ErrorPanel.svelte';
  import LoadingBlock from '$lib/components/LoadingBlock.svelte';
  import { FEATURES, FORECAST_CHANGES_DAYS } from '$lib/config';
  import { createResource } from '$lib/data/resource.svelte';
  import { addDays, daysBetween, localDate } from '$lib/transforms/dates';

  const id = $derived(page.params.id ?? '');
  const valid = $derived(isUuid(id));

  const station = createResource(async (signal) => (isUuid(page.params.id) ? getStation(page.params.id!, signal) : null));
  const today = $derived(localDate(station.data?.timezone ?? 'UTC'));

  let fromInput = $state('');
  let toInput = $state('');
  $effect(() => {
    if (station.data && !fromInput) {
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
    const st = station.data;
    if (!st || !FEATURES.forecast) return null;
    return getForecast(st.id, signal);
  });
  const changes = createResource(async (signal) => {
    const st = station.data;
    const f = from;
    const t = to;
    const problem = rangeProblem;
    const fcStatus = forecast.status;
    if (!st || !FEATURES.forecast || problem || fcStatus === 'loading') return null;
    return getForecastChanges(st.id, f, t, signal);
  });
  const pastNullProb = $derived(
    from < today && (changes.data?.days ?? []).some((d) => d.date < today && d.prob_precip_max.some((v) => v === null))
  );
</script>

<main class="page">
  {#if !valid}
    <h1>Forecast</h1>
    <p>That isn't available.</p>
    <p><a href="/">Back to the dashboard</a></p>
  {:else if !FEATURES.forecast}
    <p>Forecasts are not enabled.</p>
  {:else if station.status === 'loading'}
    <LoadingBlock />
  {:else if station.status === 'error'}
    <ErrorPanel error={station.error} onretry={() => station.reload()} />
    <p><a href="/">Back to the dashboard</a></p>
  {:else if station.data}
    <h1>{station.data.name}: forecast</h1>
    <p><a href={'/stations/' + station.data.id}>Back to the station</a></p>

    <h2>Live forecast</h2>
    {#if forecast.status === 'loading'}
      <LoadingBlock label="Loading forecast..." />
    {:else if forecast.status === 'error'}
      <ErrorPanel error={forecast.error} onretry={() => forecast.reload()} />
    {:else if forecast.data}
      <ForecastChart forecast={forecast.data} />
    {/if}

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
  {/if}
</main>

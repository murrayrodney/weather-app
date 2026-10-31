<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { clearYtdCache } from '$lib/api/cache';
  import { getStations, getYtd } from '$lib/api/endpoints';
  import { ApiError } from '$lib/api/errors';
  import EmptyState from '$lib/components/EmptyState.svelte';
  import ErrorPanel from '$lib/components/ErrorPanel.svelte';
  import ForecastRow from '$lib/components/ForecastRow.svelte';
  import GddSettings from '$lib/components/GddSettings.svelte';
  import LoadingBlock from '$lib/components/LoadingBlock.svelte';
  import SeasonToggle from '$lib/components/SeasonToggle.svelte';
  import StationPicker from '$lib/components/StationPicker.svelte';
  import StationYtdView from '$lib/components/StationYtdView.svelte';
  import { DEFAULT_SEASON, FULL_YEAR, YTD_MAX_STATIONS } from '$lib/config';
  import { createResource } from '$lib/data/resource.svelte';
  import type { GddSettings as Gdd, Station, YtdResponse } from '$lib/api/types';
  import { zoneDate } from '$lib/transforms/dates';
  import { parseGddSettings, toSearchParams } from '$lib/transforms/gddSettings';
  import { offSeasonLine } from '$lib/transforms/provenance';
  import { seriesMax } from '$lib/transforms/scale';
  import { displaySeason } from '$lib/transforms/season';
  import type { Metric } from '$lib/transforms/slots';

  let { data } = $props();

  // Forecast is the default tab (proposal 07); Rain and GDD keep their column charts.
  let view = $state<'forecast' | Metric>('forecast');
  const metric = $derived<Metric>(view === 'gdd' ? 'gdd' : 'rain');
  let range = $state<'season' | 'full'>('season');
  let sameScale = $state(false);
  let picked = $state<string[] | null>(null);

  const stations = createResource((signal) => getStations(signal));
  const gdd = $derived(parseGddSettings(page.url.searchParams, range === 'season' ? DEFAULT_SEASON.end : FULL_YEAR.end));

  const selected = $derived.by(() => {
    const all = stations.data ?? [];
    const ids = picked ?? all.map((s) => s.id);
    return ids.filter((id) => all.some((s) => s.id === id)).slice(0, YTD_MAX_STATIONS);
  });
  const selectedStations = $derived(
    selected.map((id) => (stations.data ?? []).find((s) => s.id === id)).filter((s): s is Station => s !== undefined)
  );
  const win = $derived(range === 'season' ? DEFAULT_SEASON : FULL_YEAR);
  const season = $derived.by(() => {
    const first = (stations.data ?? []).find((s) => s.id === selected[0]);
    return displaySeason(zoneDate(first?.timezone ?? 'UTC'));
  });

  const ytd = createResource<YtdResponse | null>(async (signal) => {
    // Everything reactive is read synchronously here so the resource reloads when it changes.
    const ids = selected;
    const w = win;
    const g = gdd;
    const year = season.year;
    const onColumns = view !== 'forecast'; // the forecast tab fetches /forecast per row instead
    if (ids.length === 0 || !onColumns) return null;
    return getYtd({ stationIds: ids, year, start: w.start, end: w.end, gdd: g }, signal);
  });

  function setGdd(next: Gdd) {
    const sp = toSearchParams(next).toString();
    void goto(sp ? '?' + sp : page.url.pathname, { replaceState: true, keepFocus: true, noScroll: true });
  }

  function reloadAll() {
    clearYtdCache();
    stations.reload();
    ytd.reload();
  }

  const yMax = $derived.by(() => {
    if (!sameScale || !ytd.data) return undefined;
    const m = Math.max(0, ...ytd.data.stations.map((s) => seriesMax(s, metric)));
    return m > 0 ? m * 1.05 : undefined;
  });
  const colClass = $derived('cols-' + Math.min(Math.max(selected.length, 1), 5));
  const notFound = $derived(ytd.status === 'error' && ytd.error instanceof ApiError && ytd.error.status === 404);
</script>

<main class="page">
  <h1>Dashboard</h1>
  {#if stations.status === 'loading'}
    <LoadingBlock />
  {:else if stations.status === 'error'}
    <ErrorPanel error={stations.error} onretry={() => stations.reload()} />
  {:else if (stations.data ?? []).length === 0}
    <EmptyState />
  {:else}
    <div class="controls">
      <div class="tabs" role="group" aria-label="View">
        <button type="button" aria-pressed={view === 'forecast'} onclick={() => (view = 'forecast')}>Forecast</button>
        <button type="button" aria-pressed={view === 'rain'} onclick={() => (view = 'rain')}>Rain</button>
        <button type="button" aria-pressed={view === 'gdd'} onclick={() => (view = 'gdd')}>GDD</button>
      </div>
      {#if view !== 'forecast'}
        <SeasonToggle value={range} onchange={(v) => (range = v)} />
        <label class="small"><input type="checkbox" bind:checked={sameScale} /> Same scale for all stations</label>
      {/if}
      <StationPicker stations={stations.data ?? []} {selected} onchange={(ids) => (picked = ids)} />
    </div>
    {#if view === 'gdd'}<GddSettings value={gdd} onchange={setGdd} windowEnd={win.end} />{/if}

    {#if view !== 'forecast' && season.offSeason}
      <p class="banner" data-testid="off-season-banner">{offSeasonLine(season.year)}</p>
    {/if}

    {#if view === 'forecast'}
      <div class="forecast-rows">
        {#each selectedStations as s (s.id)}
          <ForecastRow station={s} />
        {/each}
      </div>
    {:else if ytd.status === 'loading'}
      <LoadingBlock label="Loading charts..." />
    {:else if notFound}
      <ErrorPanel error={ytd.error} onretry={reloadAll} />
    {:else if ytd.status === 'error'}
      <ErrorPanel error={ytd.error} onretry={() => ytd.reload()} />
    {:else if ytd.data}
      {@const resp = ytd.data}
      <div class={'grid ' + colClass}>
        {#each resp.stations as st (st.station_id)}
          {@const station = (stations.data ?? []).find((s) => s.id === st.station_id)}
          {#if station}
            <StationYtdView
              {resp}
              {st}
              {station}
              {metric}
              {win}
              offSeason={season.offSeason}
              credentialStatus={data.me.credential?.status ?? null}
              yDomain={yMax}
              onbaseline={reloadAll}
            />
          {/if}
        {/each}
      </div>
    {/if}
  {/if}
</main>

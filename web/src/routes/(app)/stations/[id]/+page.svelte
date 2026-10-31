<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { clearYtdCache } from '$lib/api/cache';
  import { getStation, getYtd } from '$lib/api/endpoints';
  import { isUuid } from '$lib/api/ids';
  import type { GddSettings as Gdd, YtdResponse } from '$lib/api/types';
  import BaselinePicker from '$lib/components/BaselinePicker.svelte';
  import ErrorPanel from '$lib/components/ErrorPanel.svelte';
  import GddSettings from '$lib/components/GddSettings.svelte';
  import LoadingBlock from '$lib/components/LoadingBlock.svelte';
  import SeasonToggle from '$lib/components/SeasonToggle.svelte';
  import StationForecast from '$lib/components/StationForecast.svelte';
  import StationYtdView from '$lib/components/StationYtdView.svelte';
  import { DEFAULT_SEASON, FEATURES, FULL_YEAR } from '$lib/config';
  import { createResource } from '$lib/data/resource.svelte';
  import { labels } from '$lib/labels';
  import { zoneDate } from '$lib/transforms/dates';
  import { parseGddSettings, toSearchParams } from '$lib/transforms/gddSettings';
  import { offSeasonLine } from '$lib/transforms/provenance';
  import { displaySeason } from '$lib/transforms/season';
  import type { Metric } from '$lib/transforms/slots';

  let { data } = $props();

  const id = $derived(page.params.id ?? '');
  const valid = $derived(isUuid(id));

  let metric = $state<Metric>('rain');
  let range = $state<'season' | 'full'>('season');
  let changing = $state(false);

  const station = createResource(async (signal) => (isUuid(page.params.id) ? getStation(page.params.id!, signal) : null));
  const gdd = $derived(parseGddSettings(page.url.searchParams, range === 'season' ? DEFAULT_SEASON.end : FULL_YEAR.end));
  const win = $derived(range === 'season' ? DEFAULT_SEASON : FULL_YEAR);
  const season = $derived(displaySeason(zoneDate(station.data?.timezone ?? 'UTC')));

  const ytd = createResource<YtdResponse | null>(async (signal) => {
    const st = station.data;
    const w = win;
    const g = gdd;
    const year = season.year;
    if (!st) return null;
    return getYtd({ stationIds: [st.id], year, start: w.start, end: w.end, gdd: g }, signal);
  });

  function setGdd(next: Gdd) {
    const sp = toSearchParams(next).toString();
    void goto(sp ? '?' + sp : page.url.pathname, { replaceState: true, keepFocus: true, noScroll: true });
  }
  function baselineChanged() {
    changing = false;
    clearYtdCache();
    station.reload();
    ytd.reload();
  }
</script>

<main class="page">
  {#if !valid}
    <h1>Station</h1>
    <p>That isn't available.</p>
    <p><a href="/">Back to the dashboard</a></p>
  {:else if station.status === 'loading'}
    <LoadingBlock />
  {:else if station.status === 'error'}
    <ErrorPanel error={station.error} onretry={() => station.reload()} />
    <p><a href="/">Back to the dashboard</a></p>
  {:else if station.data}
    {@const s = station.data}
    <h1>{s.name}</h1>
    <nav class="section-nav" aria-label="Station sections">
      {#if FEATURES.forecast}
        <a href="#forecast">Forecast</a>
        <a href="#changes">Changes</a>
      {/if}
      <a href="#season">Rain and GDD</a>
    </nav>

    <StationForecast station={s} />

    <section id="season" class="station-section">
      <h2>Rain and GDD</h2>
      <div class="controls">
        <div class="tabs" role="group" aria-label="View">
          <button type="button" aria-pressed={metric === 'rain'} onclick={() => (metric = 'rain')}>Rain</button>
          <button type="button" aria-pressed={metric === 'gdd'} onclick={() => (metric = 'gdd')}>GDD</button>
        </div>
        <SeasonToggle value={range} onchange={(v) => (range = v)} />
      </div>
      {#if metric === 'gdd'}<GddSettings value={gdd} onchange={setGdd} windowEnd={win.end} />{/if}
      {#if season.offSeason}<p class="banner" data-testid="off-season-banner">{offSeasonLine(season.year)}</p>{/if}

      {#if ytd.status === 'loading'}
        <LoadingBlock label="Loading charts..." />
      {:else if ytd.status === 'error'}
        <ErrorPanel error={ytd.error} onretry={() => ytd.reload()} />
      {:else if ytd.data && ytd.data.stations[0]}
        <div class="detail">
          <StationYtdView
            resp={ytd.data}
            st={ytd.data.stations[0]}
            station={s}
            {metric}
            {win}
            offSeason={season.offSeason}
            credentialStatus={data.me.credential?.status ?? null}
            onbaseline={baselineChanged}
          />
        </div>
      {/if}

      <section class="card baseline-panel" data-testid="baseline-panel">
        <h2>Baseline</h2>
        {#if s.baseline}
          <p>
            Baseline: {s.baseline.name} ({s.baseline.reference_station_id}), {labels.baselineSource(s.baseline.source)}
            {#if !s.baseline.history_loaded}<span class="muted"> (history loading)</span>{/if}
          </p>
        {:else}
          <p>No baseline is set for this station yet.</p>
        {/if}
        {#if s.is_own}
          {#if !changing}
            <button type="button" class="secondary" onclick={() => (changing = true)}>{s.baseline ? 'Change baseline' : 'Choose a baseline'}</button>
          {:else}
            <BaselinePicker stationId={s.id} stationName={s.name} ondone={baselineChanged} onskip={() => (changing = false)} />
          {/if}
        {/if}
      </section>
    </section>
  {/if}
</main>

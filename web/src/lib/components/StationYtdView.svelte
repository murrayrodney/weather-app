<script lang="ts">
  import type { Station, YtdResponse, YtdStation } from '$lib/api/types';
  import CumulativeChart from '$lib/charts/CumulativeChart.svelte';
  import { localMd } from '$lib/transforms/dates';
  import { positionFor } from '$lib/transforms/position';
  import { baselineViewState, buildProvenanceLines } from '$lib/transforms/provenance';
  import type { Metric } from '$lib/transforms/slots';
  import BaselinePicker from './BaselinePicker.svelte';
  import NeedsReconnectBanner from './NeedsReconnectBanner.svelte';
  import StationCard from './StationCard.svelte';

  let {
    resp,
    st,
    station,
    metric,
    win,
    offSeason,
    credentialStatus = null,
    yDomain,
    onbaseline,
    title
  }: {
    resp: YtdResponse;
    st: YtdStation;
    station: Station;
    metric: Metric;
    win: { start: string; end: string };
    offSeason: boolean;
    credentialStatus?: string | null;
    yDomain?: number;
    onbaseline?: () => void;
    title?: string;
  } = $props();

  let picking = $state(false);

  // The metric's own window: rain uses the season window, GDD counts from the echoed gdd.start.
  const metricWin = $derived(metric === 'gdd' ? { start: resp.gdd.start, end: win.end } : win);
  const provenance = $derived(buildProvenanceLines(resp, st, metric, metricWin, { offSeason }));
  const view = $derived(baselineViewState(st));
  const position = $derived(positionFor(st, metric, metricWin.start));
  const seasonNote = $derived.by(() => {
    if (st.current.last_date !== null) return null;
    if (offSeason) return `No data for this station in the ${resp.year} season.`;
    return localMd(station.timezone) < metricWin.start ? 'Season not started. Showing prior years only.' : 'No data yet this season.';
  });
  const valueLabel = $derived(metric === 'rain' ? 'in' : 'GDD');
</script>

<div class="card station-ytd" data-testid="station-ytd">
  <StationCard {station} {credentialStatus} />
  {#if station.is_own && credentialStatus === 'invalid'}
    <NeedsReconnectBanner lastObservationAt={station.latest_observation_at} timezone={station.timezone} />
  {/if}
  {#if seasonNote}<p class="state-note" data-testid="season-note">{seasonNote}</p>{/if}
  <CumulativeChart ytd={st} {metric} window={metricWin} {valueLabel} {yDomain} {provenance} {title} />
  {#if position}<p class="position" data-testid="position">This year is {position}.</p>{/if}
  {#if station.is_own && view === 'none'}
    {#if !picking}
      <button type="button" class="secondary" onclick={() => (picking = true)}>Choose a baseline</button>
    {/if}
  {/if}
  {#if picking}
    <BaselinePicker
      stationId={station.id}
      stationName={station.name}
      ondone={() => {
        picking = false;
        onbaseline?.();
      }}
      onskip={() => (picking = false)}
    />
  {/if}
</div>

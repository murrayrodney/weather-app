<script lang="ts">
  import type { Forecast } from '$lib/api/types';
  import { FORECAST_HOURLY_DAYS } from '$lib/config';
  import { formatInZone, formatDateShort } from '$lib/transforms/dates';
  import { dailyBandSeries, forecastBand, hourlyWindow, lineFrom } from '$lib/transforms/forecast';
  import ChartFrame from './ChartFrame.svelte';
  import { H, M, W, linePath, makeScales, ribbonPath } from './scales';

  let { forecast }: { forecast: Forecast } = $props();

  let variable = $state<'temp' | 'precip'>('temp');
  let horizon = $state<'hourly' | 'daily'>('hourly');

  const hw = $derived(hourlyWindow(forecast, FORECAST_HOURLY_DAYS));
  const det = $derived(forecast.deterministic);
  const ens = $derived(forecast.ensemble);

  interface Layer {
    cls: string;
    d: string;
  }

  const model = $derived.by(() => {
    const n = horizon === 'hourly' ? hw.time.length : Math.max(det?.daily.length ?? 0, ens?.daily.length ?? 0);
    const lines: { cls: string; vals: (number | null)[] }[] = [];
    const bands: { lower: (number | null)[]; upper: (number | null)[]; median: (number | null)[] }[] = [];
    if (horizon === 'hourly') {
      if (variable === 'temp') {
        if (hw.deterministic) lines.push({ cls: 'line-forecast', vals: hw.deterministic.temperature_f });
        if (hw.ensemble) bands.push({ lower: hw.ensemble.temperature_f.p10, upper: hw.ensemble.temperature_f.p90, median: hw.ensemble.temperature_f.p50 });
      } else {
        if (hw.deterministic) lines.push({ cls: 'line-forecast', vals: hw.deterministic.precip_in });
        if (hw.ensemble) bands.push({ lower: hw.ensemble.precip_in.p10, upper: hw.ensemble.precip_in.p90, median: hw.ensemble.precip_in.p50 });
      }
    } else if (variable === 'temp') {
      if (det) {
        lines.push({ cls: 'line-forecast', vals: det.daily.map((d) => d.tmax_f) });
        lines.push({ cls: 'line-forecast', vals: det.daily.map((d) => d.tmin_f) });
      }
      if (ens) {
        const hi = dailyBandSeries(ens.daily.map((d) => d.tmax_f));
        const lo = dailyBandSeries(ens.daily.map((d) => d.tmin_f));
        bands.push({ lower: hi.p10, upper: hi.p90, median: hi.p50 }, { lower: lo.p10, upper: lo.p90, median: lo.p50 });
      }
    } else if (ens) {
      const s = dailyBandSeries(ens.daily.map((d) => d.precip_in));
      bands.push({ lower: s.p10, upper: s.p90, median: s.p50 });
    } else if (det) {
      lines.push({ cls: 'line-forecast', vals: det.daily.map((d) => d.precip_in) });
    }
    return { n, lines, bands };
  });

  const yRange = $derived.by(() => {
    let lo = Infinity;
    let hi = -Infinity;
    const eat = (a: (number | null)[]) => {
      for (const v of a) {
        if (v === null) continue;
        lo = Math.min(lo, v);
        hi = Math.max(hi, v);
      }
    };
    for (const l of model.lines) eat(l.vals);
    for (const b of model.bands) {
      eat(b.lower);
      eat(b.upper);
    }
    if (!Number.isFinite(lo)) return { lo: 0, hi: 1 };
    return { lo: variable === 'precip' ? 0 : Math.floor(lo - 2), hi: hi + (variable === 'precip' ? Math.max(0.05, hi * 0.1) : 2) };
  });
  const sc = $derived.by(() => {
    const s = makeScales(model.n, yRange.hi);
    s.y.domain([yRange.lo, yRange.hi]).nice();
    return s;
  });
  const xs = $derived((v: number) => sc.x(v));
  const ys = $derived((v: number) => sc.y(v));

  const layers = $derived.by(() => {
    const out: Layer[] = [];
    for (const b of model.bands) {
      const f = forecastBand({ p10: b.lower, p50: b.median, p90: b.upper });
      out.push({ cls: 'band-outer', d: ribbonPath(f.lower, f.upper, xs, ys) });
      out.push({ cls: 'line-median', d: linePath(f.median, xs, ys) });
    }
    for (const l of model.lines) out.push({ cls: l.cls, d: linePath(lineFrom(l.vals), xs, ys) });
    return out;
  });

  const xTicks = $derived.by(() => {
    const out: { x: number; label: string }[] = [];
    if (horizon === 'hourly') {
      hw.time.forEach((t, i) => {
        if (i % 24 === 0) out.push({ x: i, label: formatInZone(t, forecast.timezone).split(',').slice(0, 2).join(',') });
      });
    } else {
      const days = det?.daily ?? ens?.daily ?? [];
      days.forEach((d, i) => {
        if (i % 2 === 0) out.push({ x: i, label: formatDateShort(d.date) });
      });
    }
    return out;
  });
  const yTicks = $derived(sc.y.ticks(5));

  const probs = $derived.by(() => {
    if (horizon !== 'daily' || variable !== 'precip') return [];
    const n = Math.max(det?.daily.length ?? 0, ens?.daily.length ?? 0);
    return Array.from({ length: n }, (_, i) => ens?.daily[i]?.prob_precip ?? det?.daily[i]?.prob_precip_max ?? null);
  });
  const dailyPrecipBars = $derived.by(() => {
    if (horizon !== 'daily' || variable !== 'precip' || !ens) return [];
    return ens.daily.map((d, i) => ({ x: xs(i), lo: d.precip_in.p10, hi: d.precip_in.p90, mid: d.precip_in.p50 }));
  });

  const provenance = $derived([
    `Forecast: ${det?.model ?? ens?.model ?? 'model unavailable'}${ens ? `; ensemble ${ens.model}, ${ens.members} members, p10-p90 range` : ''}. Times in ${forecast.timezone}.`,
    forecast.attribution
  ]);
</script>

<div class="controls">
  <div class="tabs" role="group" aria-label="Variable">
    <button type="button" aria-pressed={variable === 'temp'} onclick={() => (variable = 'temp')}>Temperature</button>
    <button type="button" aria-pressed={variable === 'precip'} onclick={() => (variable = 'precip')}>Precipitation</button>
  </div>
  <div class="tabs" role="group" aria-label="Horizon">
    <button type="button" aria-pressed={horizon === 'hourly'} onclick={() => (horizon = 'hourly')}>Hourly (7 days)</button>
    <button type="button" aria-pressed={horizon === 'daily'} onclick={() => (horizon = 'daily')}>Daily (15 days)</button>
  </div>
</div>
{#if forecast.stale}
  <p class="banner warn" data-testid="stale-note">Showing the forecast saved at {formatInZone(forecast.generated_at, forecast.timezone)}; refreshing failed.</p>
{/if}
{#if !ens}<p class="state-note" data-testid="no-ensemble">Uncertainty range unavailable right now.</p>{/if}
{#if !det}<p class="state-note">The single-model forecast line is unavailable right now.</p>{/if}
<ChartFrame title={variable === 'temp' ? 'Temperature (F)' : 'Precipitation (in)'} {provenance}>
  <svg class="chart-svg" viewBox={`0 0 ${W} ${H}`} role="img" aria-label="Forecast chart" data-testid="forecast-chart">
    {#each yTicks as t (t)}
      <line class="grid-line" x1={M.l} x2={W - M.r} y1={ys(t)} y2={ys(t)} />
      <text x={M.l - 6} y={ys(t) + 4} text-anchor="end">{t}</text>
    {/each}
    {#each xTicks as t (t.x)}<text x={xs(t.x)} y={H - 8}>{t.label}</text>{/each}
    {#each layers as l, i (i)}<path class={l.cls} d={l.d} />{/each}
    {#each dailyPrecipBars as b, i (i)}
      {#if b.lo !== null && b.hi !== null}<line class="bar-range" x1={b.x} x2={b.x} y1={ys(b.lo)} y2={ys(b.hi)} />{/if}
      {#if b.mid !== null}<circle class="marker" cx={b.x} cy={ys(b.mid)} r="3" />{/if}
    {/each}
    {#each probs as p, i (i)}
      {#if p !== null}<text x={xs(i)} y={H - 18} text-anchor="middle">{Math.round(p)}%</text>{/if}
    {/each}
  </svg>
</ChartFrame>

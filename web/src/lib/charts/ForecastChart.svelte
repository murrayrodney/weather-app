<script lang="ts">
  import type { Forecast } from '$lib/api/types';
  import ForecastNotes from '$lib/components/ForecastNotes.svelte';
  import { formatDateShort } from '$lib/transforms/dates';
  import { dailyBandSeries, forecastBand, iconFor, lineFrom } from '$lib/transforms/forecast';
  import ChartFrame from './ChartFrame.svelte';
  import ChartLegend from './ChartLegend.svelte';
  import DayStrip from './DayStrip.svelte';
  import ChartTooltip from './ChartTooltip.svelte';
  import ForecastStack from './ForecastStack.svelte';
  import { legendLayout, type LegendItem } from './legend';
  import { M, linePath, makeScales, ribbonPath } from './scales';
  import type { StripCell } from './stackLayout';
  import { dailyPrecipRows, dailyTempRows } from './tipRows';
  import { useCrosshair } from './useCrosshair.svelte';
  import { tipDate, tipText, type TipBox } from './tooltip';

  // Station page chart. Hourly is the WU-style stack (ForecastStack). Daily keeps the daily ribbons,
  // with the Temperature | Precipitation switch (proposal 07 phase 3, section 2).
  let { forecast }: { forecast: Forecast } = $props();

  const DAILY_HEIGHT = 280;
  const MAX_WIDTH = 1400;
  let measured = $state(0);
  const width = $derived(Math.min(MAX_WIDTH, measured || 900));

  let variable = $state<'temp' | 'precip'>('temp');
  let horizon = $state<'hourly' | 'daily'>('hourly');

  const det = $derived(forecast.deterministic);
  const ens = $derived(forecast.ensemble);

  interface Layer {
    cls: string;
    d: string;
  }

  // High (gold) and low (blue) are separate lines with their own bands and dash styles, so they can be told apart.
  const model = $derived.by(() => {
    const n = Math.max(det?.daily.length ?? 0, ens?.daily.length ?? 0);
    const lines: { cls: string; vals: (number | null)[] }[] = [];
    const bands: { cls: string; lower: (number | null)[]; upper: (number | null)[]; median: (number | null)[] }[] = [];
    if (variable === 'temp') {
      if (det) {
        lines.push({ cls: 'line-tmax', vals: det.daily.map((d) => d.tmax_f) });
        lines.push({ cls: 'line-tmin', vals: det.daily.map((d) => d.tmin_f) });
      }
      if (ens) {
        const hi = dailyBandSeries(ens.daily.map((d) => d.tmax_f));
        const lo = dailyBandSeries(ens.daily.map((d) => d.tmin_f));
        bands.push(
          { cls: 'band-tmax', lower: hi.p10, upper: hi.p90, median: hi.p50 },
          { cls: 'band-tmin', lower: lo.p10, upper: lo.p90, median: lo.p50 }
        );
      }
    } else if (ens) {
      const s = dailyBandSeries(ens.daily.map((d) => d.precip_in));
      bands.push({ cls: 'band-outer', lower: s.p10, upper: s.p90, median: s.p50 });
    } else if (det) {
      lines.push({ cls: 'line-forecast', vals: det.daily.map((d) => d.precip_in) });
    }
    return { n, lines, bands };
  });

  // Legend items mirror what model and dailyPrecipBars draw (chance of rain is text labels, so it has no swatch).
  const legendItems = $derived.by((): LegendItem[] => {
    const out: LegendItem[] = [];
    if (variable === 'temp') {
      if (det) out.push({ label: 'High', mark: 'line', cls: 'line-tmax' }, { label: 'Low', mark: 'line', cls: 'line-tmin' });
      if (ens) {
        out.push(
          { label: 'High p10-p90', mark: 'band', cls: 'band-tmax' },
          { label: 'Low p10-p90', mark: 'band', cls: 'band-tmin' },
          { label: 'Median', mark: 'line', cls: 'line-median' }
        );
      }
    } else if (ens) {
      out.push(
        { label: 'Range p10-p90', mark: 'band', cls: 'band-outer' },
        { label: 'Median', mark: 'line', cls: 'line-median' },
        { label: 'Daily range', mark: 'line', cls: 'bar-range' }
      );
    } else if (det) {
      out.push({ label: 'Precipitation', mark: 'line', cls: 'line-forecast' });
    }
    return out;
  });
  // The legend spans the plot (between its left and right margins), so no item runs past the plot's right edge.
  const legendWidth = $derived(width - M.l - M.r);
  const legendHeight = $derived(legendLayout(legendItems, legendWidth).height);

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
  // Half a day's band: the first and last day centres sit inside the plot, so the day strip cells are not clipped.
  const half = $derived((width - M.l - M.r) / (2 * Math.max(model.n, 1)));
  const sc = $derived.by(() => {
    const s = makeScales(model.n, yRange.hi, width, DAILY_HEIGHT);
    s.y.domain([yRange.lo, yRange.hi]).nice();
    s.x.range([M.l + half, width - M.r - half]);
    return s;
  });
  const xs = $derived((v: number) => sc.x(v));
  const ys = $derived((v: number) => sc.y(v));

  // Hover or arrow keys pick one day (proposal 08 section 2.5): the crosshair snaps to it, and the tooltip and readout
  // name its values. The tooltip follows the active tab: temperature rows on Temperature, precipitation rows on Precipitation.
  const days = $derived(det?.daily ?? ens?.daily ?? []);
  const xh = useCrosshair({ count: () => model.n, stepAt: (px) => sc.x.invert(px), xAt: (i) => sc.x(i) });
  const hover = $derived(xh.index);
  const hoverPx = $derived(xh.px);
  function pointerPx(e: PointerEvent): number {
    const box = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
    return ((e.clientX - box.left) * width) / box.width;
  }
  const tip = $derived.by((): TipBox | null => {
    const i = hover;
    const day = i === null ? undefined : days[i];
    if (i === null || !day) return null;
    const rows = variable === 'temp' ? dailyTempRows(det?.daily[i], ens?.daily[i]) : dailyPrecipRows(det?.daily[i], ens?.daily[i]);
    return { title: tipDate(day.date), rows };
  });
  const readout = $derived(tip ? tipText(tip) : 'Move the pointer across the chart, or focus it and use the left and right arrow keys, to read the values for each day.');

  /** Switches the horizon; the crosshair is cleared because it refers to the view it was drawn in. */
  function setHorizon(h: 'hourly' | 'daily') {
    horizon = h;
    xh.clear();
  }

  const layers = $derived.by(() => {
    const out: Layer[] = [];
    for (const b of model.bands) {
      const f = forecastBand({ p10: b.lower, p50: b.median, p90: b.upper });
      out.push({ cls: b.cls, d: ribbonPath(f.lower, f.upper, xs, ys) });
      out.push({ cls: 'line-median', d: linePath(f.median, xs, ys) });
    }
    for (const l of model.lines) out.push({ cls: l.cls, d: linePath(lineFrom(l.vals), xs, ys) });
    return out;
  });

  const xTicks = $derived.by(() => {
    const days = det?.daily ?? ens?.daily ?? [];
    return days.flatMap((d, i) => (i % 2 === 0 ? [{ x: i, label: formatDateShort(d.date) }] : []));
  });
  const yTicks = $derived(sc.y.ticks(5));

  // Day strip above the daily ribbons (proposal 07 section 2, "Hourly vs daily"): one cell per daily row, centred on its x.
  const dayStrip = $derived.by((): StripCell[] => {
    const days = det?.daily ?? [];
    return days.map((d, i) => {
      const icon = iconFor(d.weather_code);
      const cx = sc.x(i);
      return {
        key: d.date,
        date: formatDateShort(d.date),
        x0: cx - half,
        x1: cx + half,
        cx,
        label: icon?.label ?? null,
        icon: icon?.kind ?? null,
        hi: d.tmax_f,
        lo: d.tmin_f,
        precip: d.precip_in,
        snow: d.snow_in
      };
    });
  });

  const probs = $derived.by(() => {
    if (variable !== 'precip') return [];
    const n = Math.max(det?.daily.length ?? 0, ens?.daily.length ?? 0);
    return Array.from({ length: n }, (_, i) => ens?.daily[i]?.prob_precip ?? det?.daily[i]?.prob_precip_max ?? null);
  });
  const dailyPrecipBars = $derived.by(() => {
    if (variable !== 'precip' || !ens) return [];
    return ens.daily.map((d, i) => ({ x: xs(i), lo: d.precip_in.p10, hi: d.precip_in.p90, mid: d.precip_in.p50 }));
  });

  const provenance = $derived([
    `Forecast: ${det?.model ?? ens?.model ?? 'model unavailable'}${ens ? `; ensemble ${ens.model}, ${ens.members} members, p10-p90 range` : ''}. Times in ${forecast.timezone}.`,
    forecast.attribution
  ]);
</script>

<div class="controls">
  <div class="tabs" role="group" aria-label="Horizon">
    <button type="button" aria-pressed={horizon === 'hourly'} onclick={() => setHorizon('hourly')}>Hourly (7 days)</button>
    <button type="button" aria-pressed={horizon === 'daily'} onclick={() => setHorizon('daily')}>Daily (15 days)</button>
  </div>
  {#if horizon === 'daily'}
    <div class="tabs" role="group" aria-label="Variable">
      <button type="button" aria-pressed={variable === 'temp'} onclick={() => (variable = 'temp')}>Temperature</button>
      <button type="button" aria-pressed={variable === 'precip'} onclick={() => (variable = 'precip')}>Precipitation</button>
    </div>
  {/if}
</div>

{#if horizon === 'hourly'}
  <ForecastStack {forecast} />
{:else}
  <div class="fit" bind:clientWidth={measured}>
    <ForecastNotes {forecast} />
    <ChartFrame title={variable === 'temp' ? 'Temperature (F), daily' : 'Precipitation (in), daily'} {provenance}>
      <DayStrip {width} cells={dayStrip} />
      {#if legendHeight > 0}
        <svg class="chart-svg plot legend-row" {width} height={legendHeight} aria-hidden="true">
          <ChartLegend items={legendItems} x={M.l} y={0} width={legendWidth} testid="legend-daily" />
        </svg>
      {/if}
      <svg
        class="chart-svg plot"
        {width}
        height={DAILY_HEIGHT}
        role="slider"
        tabindex="0"
        aria-label="Daily forecast chart"
        aria-valuemin={0}
        aria-valuemax={Math.max(model.n - 1, 0)}
        aria-valuenow={hover ?? undefined}
        aria-valuetext={readout}
        data-testid="forecast-chart"
        onpointermove={(e) => xh.onhover(pointerPx(e))}
        onpointerleave={() => xh.onhover(null)}
        onkeydown={xh.onkey}
      >
        {#each yTicks as t (t)}
          <line class="grid-line" x1={sc.x(0)} x2={width - 10} y1={ys(t)} y2={ys(t)} />
          <text x={sc.x(0) - 8} y={ys(t) + 4} text-anchor="end">{t}</text>
        {/each}
        {#each xTicks as t (t.x)}<text x={xs(t.x)} y={DAILY_HEIGHT - 8} text-anchor="middle">{t.label}</text>{/each}
        {#each layers as l, i (i)}<path class={l.cls} d={l.d} />{/each}
        {#each dailyPrecipBars as b, i (i)}
          {#if b.lo !== null && b.hi !== null}<line class="bar-range" x1={b.x} x2={b.x} y1={ys(b.lo)} y2={ys(b.hi)} />{/if}
          {#if b.mid !== null}<circle class="marker" cx={b.x} cy={ys(b.mid)} r="3" />{/if}
        {/each}
        {#each probs as p, i (i)}
          {#if p !== null}<text x={xs(i)} y={DAILY_HEIGHT - 22} text-anchor="middle">{Math.round(p)}%</text>{/if}
        {/each}
        {#if hoverPx !== null}
          <line class="crosshair" x1={hoverPx} x2={hoverPx} y1={M.t} y2={DAILY_HEIGHT - M.b} />
        {/if}
        {#if hoverPx !== null && tip}
          <ChartTooltip box={tip} anchorX={hoverPx} plotLeft={M.l} plotRight={width - M.r} top={M.t} bottom={DAILY_HEIGHT - M.b} testid="tip-daily" />
        {/if}
      </svg>
      <p class="readout" data-testid="daily-readout">{readout}</p>
    </ChartFrame>
  </div>
{/if}

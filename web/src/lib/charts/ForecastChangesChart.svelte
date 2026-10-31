<script lang="ts">
  import type { ForecastChanges } from '$lib/api/types';
  import { formatDateShort } from '$lib/transforms/dates';
  import { leadLabel, leadSeries, spreadAcrossLeads, type ChangeMetric } from '$lib/transforms/forecast';
  import ChartFrame from './ChartFrame.svelte';
  import ChartLegend from './ChartLegend.svelte';
  import ChartTooltip from './ChartTooltip.svelte';
  import { legendLayout, type LegendItem } from './legend';
  import { H, M, linePath, makeScales } from './scales';
  import { changeRows } from './tipRows';
  import { useCrosshair } from './useCrosshair.svelte';
  import { tipDate, tipText, type TipBox } from './tooltip';

  let { changes, pastNullProb = false }: { changes: ForecastChanges; pastNullProb?: boolean } = $props();

  // Measured width, drawn at 1:1 pixels, so labels keep their 12 px size at any container width.
  const MAX_WIDTH = 1400;
  let measured = $state(0);
  const width = $derived(Math.min(MAX_WIDTH, measured || 900));

  let metric = $state<ChangeMetric>('precip_in');
  const METRICS: { id: ChangeMetric; label: string }[] = [
    { id: 'precip_in', label: 'Rain' },
    { id: 'prob_precip_max', label: 'Chance of rain' },
    { id: 'tmax_f', label: 'High' },
    { id: 'tmin_f', label: 'Low' }
  ];

  const series = $derived(leadSeries(changes, metric));
  const yRange = $derived.by(() => {
    let lo = Infinity;
    let hi = -Infinity;
    for (const s of series) {
      for (const p of s) {
        if (!p) continue;
        lo = Math.min(lo, p.y);
        hi = Math.max(hi, p.y);
      }
    }
    if (!Number.isFinite(lo)) return { lo: 0, hi: 1 };
    if (metric === 'precip_in' || metric === 'prob_precip_max') return { lo: 0, hi: Math.max(hi * 1.1, metric === 'precip_in' ? 0.05 : 10) };
    return { lo: Math.floor(lo - 2), hi: hi + 2 };
  });
  const sc = $derived.by(() => {
    const s = makeScales(changes.days.length, yRange.hi, width, H);
    s.y.domain([yRange.lo, yRange.hi]).nice();
    return s;
  });
  const xs = $derived((v: number) => sc.x(v));
  const ys = $derived((v: number) => sc.y(v));
  const yTicks = $derived(sc.y.ticks(5));

  // Hover or arrow keys pick one target date (proposal 08 section 2.5). The tooltip lists every lead for that date.
  const xh = useCrosshair({ count: () => changes.days.length, stepAt: (px) => sc.x.invert(px), xAt: (i) => sc.x(i) });
  const hover = $derived(xh.index);
  const hoverPx = $derived(xh.px);
  function pointerPx(e: PointerEvent): number {
    const box = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
    return ((e.clientX - box.left) * width) / box.width;
  }
  const tip = $derived.by((): TipBox | null => {
    const i = hover;
    const day = i === null ? undefined : changes.days[i];
    if (i === null || !day) return null;
    return { title: tipDate(day.date), rows: changeRows(changes.lead_days, metric, day) };
  });
  const readout = $derived(tip ? tipText(tip) : 'Move the pointer across the chart, or focus it and use the left and right arrow keys, to read each lead for a date.');
  // One swatch per lead, drawn with the lead's own class (width and dash), so the legend matches the lines.
  const legendItems = $derived<LegendItem[]>(changes.lead_days.map((n) => ({ label: leadLabel(n), mark: 'line', cls: 'line-lead-' + n })));
  // The legend spans the plot (between its left and right margins), so no item runs past the plot's right edge.
  const legendWidth = $derived(width - M.l - M.r);
  const legendHeight = $derived(legendLayout(legendItems, legendWidth).height);
  const fmt = (v: number | null | undefined) => (v === null || v === undefined ? '-' : metric === 'precip_in' ? v.toFixed(2) : String(Math.round(v)));
  const caption = $derived([
    `Forecast: ${changes.model} as it looked 0 to 7 days before each date (day offsets, not model runs). Days are local midnight to midnight.`,
    changes.attribution,
    ...(pastNullProb ? ['Chance of rain is not available before October 2024.'] : [])
  ]);
</script>

<div class="controls">
  <div class="tabs" role="group" aria-label="Metric">
    {#each METRICS as m (m.id)}
      <button type="button" aria-pressed={metric === m.id} onclick={() => (metric = m.id)}>{m.label}</button>
    {/each}
  </div>
</div>
<ChartFrame title={`${METRICS.find((m) => m.id === metric)?.label ?? ''} by lead time`} provenance={caption}>
  <div class="fit" bind:clientWidth={measured}>
    {#if legendHeight > 0}
      <svg class="chart-svg plot legend-row" {width} height={legendHeight} aria-hidden="true">
        <ChartLegend items={legendItems} x={M.l} y={0} width={legendWidth} testid="legend-changes" />
      </svg>
    {/if}
    <svg
      class="chart-svg plot"
      {width}
      height={H}
      role="slider"
      tabindex="0"
      aria-label="Forecast changes by lead time"
      aria-valuemin={0}
      aria-valuemax={Math.max(changes.days.length - 1, 0)}
      aria-valuenow={hover ?? undefined}
      aria-valuetext={readout}
      data-testid="changes-chart"
      onpointermove={(e) => xh.onhover(pointerPx(e))}
      onpointerleave={() => xh.onhover(null)}
      onkeydown={xh.onkey}
    >
      {#each yTicks as t (t)}
        <line class="grid-line" x1={M.l} x2={width - M.r} y1={ys(t)} y2={ys(t)} />
        <text x={M.l - 6} y={ys(t) + 4} text-anchor="end">{t}</text>
      {/each}
      {#each changes.days as d, i (d.date)}
        <!-- the last label ends at the plot's right edge so it is not clipped by the SVG -->
        <text x={xs(i)} y={H - 8} text-anchor={i === changes.days.length - 1 ? 'end' : 'middle'}>{formatDateShort(d.date)}</text>
      {/each}
      {#each series as s, li (li)}
        <path class={'line-lead-' + changes.lead_days[li]} d={linePath(s, xs, ys)} />
      {/each}
      {#if hoverPx !== null}
        <line class="crosshair" x1={hoverPx} x2={hoverPx} y1={M.t} y2={H - M.b} />
      {/if}
      {#if hoverPx !== null && tip}
        <ChartTooltip box={tip} anchorX={hoverPx} plotLeft={M.l} plotRight={width - M.r} top={M.t} bottom={H - M.b} testid="tip-changes" />
      {/if}
    </svg>
    <p class="readout" data-testid="changes-readout">{readout}</p>
  </div>
  <div class="table-scroll">
    <table class="data" data-testid="changes-table">
      <thead>
        <tr>
          <th>Day</th>
          {#each changes.lead_days as n (n)}<th>{leadLabel(n)}</th>{/each}
          <th>Moved by</th>
        </tr>
      </thead>
      <tbody>
        {#each changes.days as d (d.date)}
          {@const sp = spreadAcrossLeads(d, metric)}
          <tr>
            <td>{formatDateShort(d.date)}</td>
            {#each d[metric] as v, i (i)}<td>{fmt(v)}</td>{/each}
            <td>{sp ? `moved by ${fmt(sp.min)} to ${fmt(sp.max)}` : '-'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</ChartFrame>

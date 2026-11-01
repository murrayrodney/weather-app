<script lang="ts">
  import { scaleLinear } from 'd3-scale';
  import type { ForecastChanges } from '$lib/api/types';
  import { formatDateShort } from '$lib/transforms/dates';
  import { changesToday, dayLeads, leadIssued, spreadAcrossLeads, type ChangeMetric } from '$lib/transforms/forecast';
  import ChartFrame from './ChartFrame.svelte';
  import ChartLegend from './ChartLegend.svelte';
  import ChartTooltip from './ChartTooltip.svelte';
  import FacetPanel from './FacetPanel.svelte';
  import { facetLayout, facetTitle, FACET_GAP, FACET_PAD, panelX, sharedYDomain } from './facetLayout';
  import { legendLayout, type LegendItem } from './legend';
  import { tipText, type TipBox } from './tooltip';
  import { facetRows } from './tipRows';
  import { useFacetCursor } from './useFacetCursor.svelte';

  // "How the forecast changed" (proposal 09): one panel per target day, the lead on each panel's x axis and one y
  // domain shared by every panel of the tab. The panels are one SVG, so the grid takes the measured width.
  let { changes, pastNullProb = false }: { changes: ForecastChanges; pastNullProb?: boolean } = $props();

  // Measured width, drawn at 1:1 pixels, so labels keep their 12 px size at any container width.
  const MAX_WIDTH = 1400;
  let measured = $state(0);
  const width = $derived(Math.min(MAX_WIDTH, measured || 900));

  let metric = $state<ChangeMetric>('precip_in');
  // tone: the metric's colour class, set on the SVG so the panels' marks and the legend read it (app.css).
  const METRICS: { id: ChangeMetric; label: string; tone: string }[] = [
    { id: 'precip_in', label: 'Rain', tone: 'tone-rain' },
    { id: 'prob_precip_max', label: 'Chance of rain', tone: 'tone-chance' },
    { id: 'tmax_f', label: 'High', tone: 'tone-high' },
    { id: 'tmin_f', label: 'Low', tone: 'tone-low' }
  ];
  const current = $derived(METRICS.find((m) => m.id === metric) ?? METRICS[0]);

  // Station-local today, derived once from the response (generated_at and timezone).
  const today = $derived(changesToday(changes));
  const leads = $derived(changes.days.map((d) => dayLeads(d, metric, today)));
  // All panels of the tab share one y domain, from their issued values only (proposal 09 decision 2).
  const domain = $derived(sharedYDomain(metric, leads.flat()));
  const yScale = $derived(scaleLinear().domain(domain));
  const yTicks = $derived(yScale.ticks(3));
  const yFormat = $derived(yScale.tickFormat(3));

  const grid = $derived(facetLayout(width, changes.days.length));
  const gridHeight = $derived(grid.rows ? grid.rows * grid.cellH + (grid.rows - 1) * FACET_GAP : 0);

  // One cursor for the grid: the panel and the lead slot (proposal 09 section 6). The pointer picks both; the keys move them.
  const cursor = useFacetCursor({
    panels: () => changes.days.length,
    leads: () => changes.lead_days.length,
    cols: () => grid.cols,
    rows: () => grid.rows
  });
  // The tooltip and the slider's value text describe the hovered panel: its day, the lead under the cursor and now.
  const hover = $derived.by(() => {
    const p = cursor.panel;
    const l = cursor.lead;
    if (p === null || l === null) return null;
    const cell = grid.cells[p];
    const pts = leads[p];
    if (!cell || !pts?.[l]) return null;
    const box: TipBox = { title: facetTitle(changes.days[p].date, today), rows: facetRows(metric, pts[l], pts[pts.length - 1]) };
    return { cell, x: panelX(grid.cellW, cell.col === 0, pts.length), lead: l, box };
  });
  const maxLead = $derived(Math.max(...changes.lead_days));
  const sliderLabel = $derived(
    `${current.label} forecast changes by day. Line or bars: forecast from ${maxLead} days before to latest. Dashed line: latest. Shaded: not issued yet.`
  );

  // Pointer: the grid cell under it, then the lead slot in that cell's own x scale. Empty cells in the last row pick nothing.
  function pointer(e: PointerEvent) {
    const box = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
    const x = ((e.clientX - box.left) * width) / box.width;
    const y = ((e.clientY - box.top) * gridHeight) / box.height;
    const col = Math.min(grid.cols - 1, Math.max(0, Math.floor(x / (grid.cellW + FACET_GAP))));
    const row = Math.min(grid.rows - 1, Math.max(0, Math.floor(y / (grid.cellH + FACET_GAP))));
    const index = row * grid.cols + col;
    const cell = grid.cells[index];
    if (!cell) {
      cursor.clear();
      return;
    }
    cursor.onhover(index, panelX(grid.cellW, cell.col === 0, changes.lead_days.length).stepAt(x - cell.x));
  }

  // Rain is bars from zero; the other metrics are lines (proposal 09 section 4).
  const kind = $derived<'bar' | 'line'>(metric === 'precip_in' ? 'bar' : 'line');
  const legendItems = $derived<LegendItem[]>([
    kind === 'bar' ? { label: 'Forecast', mark: 'bar', cls: 'bar-precip' } : { label: 'Forecast', mark: 'line', cls: 'line-facet' },
    { label: 'Latest', mark: 'line', cls: 'ref-latest' },
    { label: 'Not issued', mark: 'area', cls: 'facet-unissued' }
  ]);
  const legendHeight = $derived(legendLayout(legendItems, width).height);

  const fmt = (v: number | null | undefined) => (v === null || v === undefined ? '-' : metric === 'precip_in' ? v.toFixed(2) : String(Math.round(v)));
  const caption = $derived([
    `Forecast: ${changes.model}.`,
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
<ChartFrame title={current.label} provenance={caption}>
  <div class="fit" bind:clientWidth={measured}>
    {#if legendHeight > 0}
      <svg class={`chart-svg plot legend-row ${current.tone}`} {width} height={legendHeight} aria-hidden="true">
        <ChartLegend items={legendItems} x={0} y={0} {width} testid="legend-changes" />
      </svg>
    {/if}
    <svg
      class={`chart-svg plot ${current.tone}`}
      {width}
      height={gridHeight}
      role="slider"
      tabindex="0"
      aria-label={sliderLabel}
      aria-valuemin={0}
      aria-valuemax={Math.max(changes.lead_days.length - 1, 0)}
      aria-valuenow={cursor.lead ?? undefined}
      aria-valuetext={hover ? tipText(hover.box) : undefined}
      data-testid="changes-chart"
      onpointermove={pointer}
      onpointerleave={() => cursor.clear()}
      onkeydown={cursor.onkey}
    >
      {#each grid.cells as cell (cell.index)}
        {@const day = changes.days[cell.index]}
        <g transform={`translate(${cell.x} ${cell.y})`} data-testid={`facet-${day.date}`}>
          <FacetPanel
            title={facetTitle(day.date, today)}
            {kind}
            points={leads[cell.index]}
            {domain}
            ticks={yTicks}
            tickText={yFormat}
            showAxis={cell.col === 0}
            cursor={cursor.lead}
            w={grid.cellW}
            h={grid.cellH}
          />
        </g>
      {/each}
      {#if hover}
        <g transform={`translate(${hover.cell.x} ${hover.cell.y})`}>
          <ChartTooltip
            box={hover.box}
            anchorX={hover.x.xAt(hover.lead)}
            plotLeft={hover.x.pl}
            plotRight={hover.x.pr}
            top={FACET_PAD.top}
            bottom={grid.cellH - FACET_PAD.bottom}
            testid="tip-changes"
          />
        </g>
      {/if}
    </svg>
  </div>
  <div class="table-scroll">
    <table class="data" data-testid="changes-table">
      <thead>
        <tr>
          <th>Day</th>
          {#each [...changes.lead_days].reverse() as n (n)}<th>{n === 0 ? 'now' : `${n}d`}</th>{/each}
          <th>Range</th>
        </tr>
      </thead>
      <tbody>
        {#each changes.days as d (d.date)}
          {@const sp = spreadAcrossLeads(d, metric, today)}
          <tr>
            <td>{formatDateShort(d.date)}</td>
            {#each [...changes.lead_days].reverse() as n (n)}<td>{leadIssued(d.date, n, today) ? fmt(d[metric][n]) : '–'}</td>{/each}
            <td>{sp ? `${fmt(sp.min)}–${fmt(sp.max)}` : '-'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</ChartFrame>

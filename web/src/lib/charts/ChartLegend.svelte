<script lang="ts">
  import { legendLayout, LEGEND_ROW, LEGEND_SWATCH as SWATCH, type LegendItem } from './legend';
  import { WIND_ARROW } from './stackLayout';

  // Legend drawn inside a chart's SVG (proposal 08 section 1.1). Each swatch carries the series' own CSS class, so a
  // dash or colour change in app.css changes the mark and its legend together. aria-hidden: screen readers get the
  // chart's own label and its slider value text. Callers pass only the items they draw.
  let {
    items,
    x,
    y,
    width,
    testid,
    hatchId
  }: {
    items: LegendItem[];
    x: number;
    y: number;
    width: number;
    testid: string;
    hatchId?: string; // pattern id for the hatch swatch (the chart's own <pattern>)
  } = $props();

  const layout = $derived(legendLayout(items, width));
</script>

<g class="chart-legend" aria-hidden="true" data-testid={testid}>
  {#each layout.placed as p, i (i)}
    {@const sx = x + p.x}
    {@const cy = y + p.row * LEGEND_ROW + LEGEND_ROW / 2 - 1}
    {#if p.item.mark === 'line'}
      <line class={p.item.cls} x1={sx} x2={sx + SWATCH} y1={cy} y2={cy} />
    {:else if p.item.mark === 'band' || p.item.mark === 'area'}
      <rect class={p.item.cls} x={sx} y={cy - 5} width={SWATCH} height="10" />
    {:else if p.item.mark === 'bar'}
      <rect class={p.item.cls} x={sx + 6} y={cy - 6} width="10" height="12" />
    {:else if p.item.mark === 'hatch'}
      <rect class={p.item.cls} x={sx + 6} y={cy - 6} width="10" height="12" fill={hatchId ? `url(#${hatchId})` : undefined} />
    {:else if p.item.mark === 'arrow'}
      <path class={p.item.cls} transform={`translate(${sx + SWATCH / 2} ${cy}) rotate(90)`} d={WIND_ARROW} />
    {/if}
    <text class="legend-label" x={sx + SWATCH + 6} y={cy + 4}>{p.item.label}</text>
  {/each}
</g>

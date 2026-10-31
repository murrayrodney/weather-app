<script module lang="ts">
  // Each stack gets its own snow hatch <pattern> id, so several stacks on one page never share a pattern.
  let instances = 0;
</script>

<script lang="ts">
  import type { NightSpan } from '$lib/transforms/forecast';
  import ChartLegend from './ChartLegend.svelte';
  import ChartTooltip from './ChartTooltip.svelte';
  import { type Glyph, type Layer, type StackLayout, WIND_ARROW } from './stackLayout';
  import type { TipBox } from './tooltip';

  // Draws stacked panels on one time axis. Geometry comes from layoutStack; the caller builds each panel's paths.
  // Shading, grid, now line and crosshair are drawn in every panel so the stack reads as one chart. Each panel's
  // legend sits in its header row; the header is painted over the now line and crosshair so they do not run through
  // the legend text.
  let {
    layout,
    layers,
    glyphs = [],
    night,
    dayLines,
    nowPx,
    hoverPx,
    index,
    count,
    valuetext,
    label,
    tips = [],
    onhover,
    onkey
  }: {
    layout: StackLayout;
    layers: Layer[][]; // one list per panel, drawn in order
    glyphs?: Glyph[][]; // one list per panel: wind arrows
    night: NightSpan[];
    dayLines: { px: number; text: string }[]; // local midnights: vertical grid and x label
    nowPx: number | null;
    hoverPx: number | null;
    index: number | null; // the hourly step under the crosshair (keyboard position)
    count: number;
    valuetext: string;
    label: string;
    tips?: TipBox[]; // one tooltip per panel, index-aligned with layout.panels; empty means no tooltips
    onhover: (px: number | null) => void;
    onkey: (e: KeyboardEvent) => void;
  } = $props();

  instances += 1;
  const hatchId = `snow-hatch-${instances}`;

  const first = $derived(layout.panels[0]);
  const last = $derived(layout.panels[layout.panels.length - 1]);

  function pointerPx(e: PointerEvent): number {
    const box = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
    return ((e.clientX - box.left) * layout.width) / box.width;
  }
</script>

<svg
  class="chart-svg plot stack"
  width={layout.width}
  height={layout.height}
  role="slider"
  tabindex="0"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={Math.max(count - 1, 0)}
  aria-valuenow={index ?? undefined}
  aria-valuetext={valuetext}
  data-testid="panel-stack"
  onpointermove={(e) => onhover(pointerPx(e))}
  onpointerleave={() => onhover(null)}
  onkeydown={onkey}
>
  <defs>
    <pattern id={hatchId} width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
      <rect class="snow-bg" width="5" height="5" />
      <line class="snow-hatch" x1="0" y1="0" x2="0" y2="5" />
    </pattern>
  </defs>
  {#each layout.panels as p, i (i)}
    <rect class="panel-bg" x={layout.left} y={p.top} width={layout.right - layout.left} height={p.bottom - p.top} />
    {#each night as n, j (j)}
      <rect class="night" x={layout.x(n.start)} y={p.top} width={layout.x(n.end) - layout.x(n.start)} height={p.bottom - p.top} />
    {/each}
    {#each p.ticks as t (t)}
      <line class="grid-line" x1={layout.left} x2={layout.right} y1={p.y(t)} y2={p.y(t)} />
      <text class="tick-y" x={layout.left - 8} y={p.y(t) + 4} text-anchor="end">{t}</text>
    {/each}
    {#each dayLines as d, k (k)}
      <line class="day-line" x1={d.px} x2={d.px} y1={p.top} y2={p.bottom} />
    {/each}
    {#each layers[i] ?? [] as l, k (k)}<path class={l.cls} d={l.d} fill={l.hatch ? `url(#${hatchId})` : undefined} />{/each}
    {#each glyphs[i] ?? [] as g, k (k)}
      <path class="wind-arrow" transform={`translate(${g.x} ${g.y}) rotate(${g.rot})`} d={WIND_ARROW} />
    {/each}
    {#each p.right as a, k (k)}
      {#each a.ticks as t (t)}
        <text class="tick-y" x={a.x} y={a.y(t) + 4}>{t.toFixed(a.digits)}</text>
      {/each}
      <text class="panel-unit" x={a.x} y={p.top - 7}>{a.unit}</text>
    {/each}
    <text class="panel-unit" x={layout.left + 8} y={p.top + 15}>{p.unit}</text>
    <rect class="panel-frame" x={layout.left} y={p.top} width={layout.right - layout.left} height={p.bottom - p.top} />
  {/each}

  {#if nowPx !== null}
    <line class="now-line" x1={nowPx} x2={nowPx} y1={first.head - 4} y2={last.bottom} />
    <text class="now-label" x={nowPx} y={first.head - 8} text-anchor="middle">Now</text>
  {/if}
  {#if hoverPx !== null}
    <line class="crosshair" x1={hoverPx} x2={hoverPx} y1={first.top} y2={last.bottom} />
  {/if}
  {#each layout.panels as p, i (i)}
    <rect class="legend-band" x={layout.left} y={p.head} width={layout.right - layout.left} height={p.top - p.head} />
    <ChartLegend items={p.legend} x={layout.left} y={p.head} width={layout.right - layout.left} testid={`legend-${p.name}`} {hatchId} />
  {/each}
  {#each dayLines as d, k (k)}
    <text class="tick-x" x={d.px} y={layout.axisY} text-anchor="middle">{d.text}</text>
  {/each}
  {#if hoverPx !== null}
    {#each layout.panels as p, i (i)}
      {#if tips[i]}
        <ChartTooltip
          box={tips[i]}
          anchorX={hoverPx}
          plotLeft={layout.left}
          plotRight={layout.right}
          top={p.top}
          bottom={p.bottom}
          {hatchId}
          testid={`tip-${p.name}`}
        />
      {/if}
    {/each}
  {/if}
</svg>
